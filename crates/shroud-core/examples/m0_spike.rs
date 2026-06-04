//! M0 premise spike: host an onion service *in-process* (no external `tor`), self-dial it,
//! and prove the transport is real — now hardened per review findings:
//!
//!   * **S8 (ephemeral onion):** arti's state + keystore live in a temp dir wiped on exit, so a
//!     fresh `.onion` is minted each run and nothing persists across sessions.
//!   * **A4 (sustained + reconnect):** after the first echo, keep one stream open and exchange
//!     periodic traffic for a configurable duration, then re-dial on a fresh stream.
//!
//! Run: `cargo run -p shroud-core --example m0_spike`
//! Tune the sustained duration with `SHROUD_M0_SECS` (default 60; M0 exit target is >= 300).

use arti_client::config::CfgPath;
use arti_client::{DataStream, TorClient, TorClientConfig};
use futures::StreamExt;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tor_cell::relaycell::msg::Connected;
use tor_hsservice::config::OnionServiceConfigBuilder;

/// Virtual port the client dials. Arbitrary for an onion service (there is no HTTP here);
/// the service receives every stream regardless and the handler accepts them all.
const SPIKE_PORT: u16 = 9999;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    println!("Starting shroud-speak M0 spike...");

    // --- S8: ephemeral storage -------------------------------------------------------------
    // arti 0.23 has no true in-memory onion key: `launch_onion_service` cannot override the
    // KeyMgr/StateMgr for ephemeral operation (arti issue #1186). The closest honest
    // approximation is to point arti's state + cache at a fresh temp dir that is removed when
    // this process exits — a new .onion every run, nothing persisted across sessions. On
    // Linux/Termux, setting TMPDIR to a tmpfs mount keeps the key RAM-only.
    let tmp = tempfile::Builder::new().prefix("shroud-m0-").tempdir()?;
    println!("Ephemeral arti state dir: {} (wiped on exit)", tmp.path().display());

    let mut config_builder = TorClientConfig::builder();
    config_builder.address_filter().allow_onion_addrs(true);
    config_builder
        .storage()
        .state_dir(CfgPath::new_literal(tmp.path().join("state")))
        .cache_dir(CfgPath::new_literal(tmp.path().join("cache")));
    let config = config_builder.build()?;

    let tor_client = TorClient::create_bootstrapped(config).await?;
    println!("Tor client bootstrapped successfully!");

    // Vanguards / DoS hardening: compiled in (see Cargo.toml) and on by default for HS circuits
    // in arti 0.23. NOTE (review S9/C8): arti 0.23 exposes no public accessor to *assert* the
    // active vanguard state at runtime; a stronger M0 check would confirm vanguard layers from
    // circuit-construction logs.
    println!("Vanguards feature compiled in; full vanguards default-on for HS circuits.");

    let svc_cfg = OnionServiceConfigBuilder::default()
        .nickname("speak_spike".to_owned().try_into()?)
        .build()?;
    let (running_service, rend_requests) = tor_client.launch_onion_service(svc_cfg)?;
    println!("Onion service launched successfully!");

    let hsid = running_service
        .onion_name()
        .ok_or_else(|| anyhow::anyhow!("Failed to retrieve HsId from running service"))?;
    let onion_address = hsid.to_string();
    println!("\n========================================");
    println!("ONION SERVICE HOSTED!");
    println!("Address: {}", onion_address);
    println!("========================================\n");

    // --- service: accept loop; each stream echoes every message until the peer closes -------
    let mut stream_requests = tor_hsservice::handle_rend_requests(rend_requests);
    tokio::spawn(async move {
        while let Some(stream_req) = stream_requests.next().await {
            tokio::spawn(async move {
                match stream_req.accept(Connected::new_empty()).await {
                    Ok(mut data_stream) => {
                        println!("[service] stream accepted");
                        let mut buf = [0u8; 1024];
                        // Loop so a single stream can carry many messages (A4 sustained test).
                        // Ok(0) or a read error means the client closed its end — Tor signals
                        // this with an END cell, which surfaces here as EOF/Err. Treat both as
                        // a normal teardown and stop.
                        loop {
                            match data_stream.read(&mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    let response =
                                        format!("Echo: {}", String::from_utf8_lossy(&buf[..n]));
                                    if data_stream.write_all(response.as_bytes()).await.is_err() {
                                        break;
                                    }
                                    // Flush so bytes leave before any drop: Tor has no
                                    // half-close, and dropping a stream sends END/MISC, which
                                    // would discard unflushed data.
                                    if data_stream.flush().await.is_err() {
                                        break;
                                    }
                                }
                                Err(_) => break, // client gone (END/MISC) — normal teardown
                            }
                        }
                        println!("[service] stream closed");
                    }
                    Err(e) => eprintln!("[service] accept failed: {:?}", e),
                }
            });
        }
    });

    // --- client helpers --------------------------------------------------------------------

    /// Dial the onion service, retrying while the descriptor is still publishing.
    async fn dial(
        tor_client: &TorClient<impl tor_rtcompat::Runtime>,
        addr: &str,
    ) -> anyhow::Result<DataStream> {
        let max_attempts = 36; // ~3 minutes total (5s between attempts)
        for attempt in 1..=max_attempts {
            match tor_client.connect((addr, SPIKE_PORT)).await {
                Ok(stream) => {
                    println!("[client] connected on attempt {attempt}");
                    return Ok(stream);
                }
                Err(e) if attempt == max_attempts => {
                    return Err(anyhow::anyhow!("connect failed after {max_attempts}: {e:?}"));
                }
                Err(e) => {
                    println!("[client] attempt {attempt} failed (descriptor publishing): {e}; retry in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
        unreachable!("loop returns on success or on the final attempt")
    }

    /// One request/response over an already-open stream (no close in between), so we exercise
    /// a genuinely long-lived stream rather than a fresh one per message.
    async fn round_trip(stream: &mut DataStream, msg: &str) -> anyhow::Result<String> {
        stream.write_all(msg.as_bytes()).await?;
        stream.flush().await?;
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).await?;
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }

    // --- A4 phase 1: sustained traffic on one long-lived stream ----------------------------
    let hold_secs: u64 = std::env::var("SHROUD_M0_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    println!("Dialing self at {onion_address} ...");
    let mut stream = dial(&tor_client, &onion_address).await?;

    let first = round_trip(&mut stream, "Hello through Tor onion service!").await?;
    println!("[client] first echo: {first}");
    anyhow::ensure!(first == "Echo: Hello through Tor onion service!", "unexpected first echo: {first:?}");

    println!("[A4] sustained phase: periodic traffic for {hold_secs}s on a single stream");
    let start = Instant::now();
    let mut seq = 0u32;
    while start.elapsed() < Duration::from_secs(hold_secs) {
        tokio::time::sleep(Duration::from_secs(5)).await;
        seq += 1;
        let got = round_trip(&mut stream, &format!("ping {seq}")).await?;
        let want = format!("Echo: ping {seq}");
        anyhow::ensure!(got == want, "sustained mismatch at seq {seq}: got {got:?}, want {want:?}");
        println!("[A4] keepalive {seq} ok ({}s elapsed)", start.elapsed().as_secs());
    }
    drop(stream); // close the long-lived stream
    println!("[A4] sustained phase complete: {seq} keepalives over {}s", start.elapsed().as_secs());

    // --- A4 phase 2: reconnect on a fresh stream -------------------------------------------
    println!("[A4] reconnect phase: dialing a fresh stream to the same service");
    let mut stream2 = dial(&tor_client, &onion_address).await?;
    let again = round_trip(&mut stream2, "reconnected").await?;
    println!("[client] echo after reconnect: {again}");
    anyhow::ensure!(again == "Echo: reconnected", "reconnect echo mismatch: {again:?}");
    drop(stream2);

    println!(
        "\nM0 spike successful! (ephemeral onion, sustained {hold_secs}s / {seq} keepalives, reconnect verified)"
    );
    Ok(())
}

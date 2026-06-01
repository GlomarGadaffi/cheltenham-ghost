//! M0 — Premise spike: an in-process Tor onion service with **no external `tor`**.
//!
//! Bootstrap arti → launch an onion service → self-dial it from the *same* client
//! → round-trip bytes. If this runs, the core architecture is possible. This is a
//! throwaway (ROADMAP budgets ~150 lines); it deliberately does no audio, no Noise,
//! and uses the default on-disk config — the in-memory/ephemeral-key onion (the real
//! anti-forensic claim) is a later milestone.
//!
//! Run:  `RUST_LOG=info cargo run -p m0-spike`
//!
//! NOT compile-verified in this repo's environment (arti isn't vendored). API/version
//! points to confirm against your pinned arti are marked `VERIFY:` below.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use arti_client::{TorClient, TorClientConfig};
use futures::StreamExt;
use safelog::DisplayRedacted; // brings `display_unredacted()` into scope for HsId
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tor_cell::relaycell::msg::Connected;
// VERIFY: builder location. It lives in `tor-hsservice` and is usually re-exported as
// `arti_client::config::onion_service::OnionServiceConfigBuilder` too — use whichever
// your version exposes.
use tor_hsservice::config::OnionServiceConfigBuilder;

/// Virtual port the onion service listens on (arbitrary for the spike).
const SPIKE_PORT: u16 = 80;
const PING: &[u8] = b"shroud-speak M0: ping through a self-hosted onion";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    println!("== shroud-speak M0 spike: in-process onion, no external tor ==\n");

    // 1. Client config. The default persists Tor state + the HS identity key to disk
    //    (~/.local/share/arti, %APPDATA%\arti on Windows). Fine for a throwaway spike.
    //    We must also explicitly allow *connecting* to .onion addresses — arti gates
    //    that in config, not just behind the cargo feature.
    //    VERIFY: this knob has moved across versions; if absent, onion client dialing
    //    may already be on by default and this block can be dropped.
    let mut cfg_builder = TorClientConfig::builder();
    cfg_builder.address_filter().allow_onion_addrs(true);
    let config = cfg_builder.build().context("building TorClientConfig")?;

    // 2. Bootstrap the embedded Tor client. This IS arti — no child process spawns.
    println!("[1/5] bootstrapping embedded Tor client (arti)…");
    let tor_client = TorClient::create_bootstrapped(config)
        .await
        .context("bootstrapping TorClient")?;
    println!("      bootstrapped.\n");

    // 3. Launch the onion service in-process. Nickname kept strictly alphanumeric to
    //    satisfy HsNickname validation (hyphens/underscores can be rejected).
    let svc_cfg = OnionServiceConfigBuilder::default()
        .nickname("speakspike".parse().context("invalid HsNickname")?)
        .build()
        .context("building OnionServiceConfig")?;

    println!("[2/5] launching onion service…");
    // Signature differs by version: 0.42 returns Result<Option<_>> (None = the
    // onion-service feature isn't compiled in), whereas 0.23 returned the tuple
    // directly. Hence `?` then `.ok_or_else` here.
    let (service, rend_requests) = tor_client
        .launch_onion_service(svc_cfg)
        .context("launch_onion_service (needs the `onion-service-service` feature)")?
        .ok_or_else(|| anyhow!("onion-service support is not enabled in this build"))?;

    // 4. The .onion address *is* the service's public key. HsId redacts itself in
    //    Display/Debug by default for privacy (via safelog), so ask explicitly for the
    //    full address. (`onion_name()` is the deprecated alias of `onion_address()`.)
    let onion = service
        .onion_address()
        .ok_or_else(|| anyhow!("onion service has no address yet"))?;
    let onion_str = onion.display_unredacted().to_string();
    println!("      address: {onion_str}\n");

    // 5. Acceptor: turn rendezvous requests into stream requests, accept each, echo.
    let mut stream_requests = tor_hsservice::handle_rend_requests(rend_requests);
    tokio::spawn(async move {
        while let Some(stream_request) = stream_requests.next().await {
            tokio::spawn(async move {
                let served: Result<()> = async {
                    // The spike accepts everything; a real service would inspect
                    // `stream_request.request()` and only accept Begin on SPIKE_PORT.
                    let mut ds = stream_request
                        .accept(Connected::new_empty())
                        .await
                        .context("accept stream")?;
                    let mut buf = vec![0u8; 256];
                    let n = ds.read(&mut buf).await.context("service read")?;
                    println!("      [service] received {n} bytes; echoing back");
                    ds.write_all(&buf[..n]).await.context("service write")?;
                    ds.flush().await.context("service flush")?;
                    Ok(())
                }
                .await;
                if let Err(e) = served {
                    eprintln!("      [service] stream error: {e:#}");
                }
            });
        }
    });

    // 6. Self-dial from the SAME client. The first descriptor publish can take tens of
    //    seconds to minutes, so retry instead of assuming instant reachability — the
    //    draft plan dialed once immediately, which can look like a hang.
    println!("[3/5] dialing self at {onion_str}:{SPIKE_PORT} (first publish can take 30–120s)…");
    let mut stream = {
        const ATTEMPTS: u32 = 6;
        let mut got = None;
        for attempt in 1..=ATTEMPTS {
            // `(&str, u16)` already implements IntoTorAddr — pass it directly, no try_into.
            match tor_client.connect((onion_str.as_str(), SPIKE_PORT)).await {
                Ok(s) => {
                    got = Some(s);
                    break;
                }
                Err(e) => {
                    eprintln!("      dial attempt {attempt}/{ATTEMPTS} failed: {e}");
                    if attempt < ATTEMPTS {
                        tokio::time::sleep(Duration::from_secs(20)).await;
                    }
                }
            }
        }
        got.ok_or_else(|| anyhow!("self-dial failed after {ATTEMPTS} attempts"))?
    };
    println!("      connected to self over Tor.\n");

    // 7. Round-trip bytes both directions.
    println!("[4/5] sending {} bytes…", PING.len());
    stream.write_all(PING).await.context("client write")?;
    stream.flush().await.context("client flush")?;

    let mut buf = vec![0u8; 256];
    let n = stream.read(&mut buf).await.context("client read")?;
    println!("      got echo: {:?}", String::from_utf8_lossy(&buf[..n]));
    anyhow::ensure!(&buf[..n] == PING, "echo mismatch — round-trip failed");
    println!("      round-trip verified.\n");

    // 8. The M0 exit criterion plans usually skip: confirm guard-discovery / DoS
    //    defenses are actually present. Code can't fully prove it; surface what it can.
    report_security_posture();

    println!("\n[5/5] M0 OK: bytes echoed through a self-hosted onion, zero external processes.");
    drop(service); // explicit teardown
    Ok(())
}

/// Part of M0's exit criterion: vanguards / onion-service DoS hardening must be
/// present and on. This can't be fully asserted from code, so name what to verify.
fn report_security_posture() {
    println!("[security] M0 posture checks — verify against your pinned arti, don't assume:");
    println!("  - vanguards: recent arti applies guard-discovery defenses to HS circuits");
    println!("    by default. Confirm the active mode (config `vanguards.mode`); run with");
    println!("    RUST_LOG=debug to watch circuits build through the vanguard layers.");
    println!("  - onion-service DoS: check what your version exposes vs C-tor (intro-point");
    println!("    rate limiting / proof-of-work) and record the gap explicitly.");
    println!("  - the launch + self-dial above succeeding proves BOTH onion-service-service");
    println!("    and onion-service-client are compiled in and functional.");
}

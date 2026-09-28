use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use rand::Rng;

// Use small request sizes so reqwest never buffers huge response bodies and the
// allocator does not hold multi-megabyte arenas after the test finishes.
const UPLOAD_CHUNK_BYTES: usize = 1024 * 1024; // 1 MiB per request
const DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down?bytes=10485760";
const UPLOAD_URL: &str = "https://speed.cloudflare.com/__up";
const TEST_DURATION: Duration = Duration::from_secs(5);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
const RAMP_UP_SAMPLES: usize = 10; // skip first 1 second

#[derive(Debug, Clone, Copy)]
pub struct SpeedResult {
    pub peak_mbps: f64,
    pub stable_avg_mbps: f64,
}

pub async fn run() {
    tracing::info!(
        "Running startup speed test (download {}s + upload {}s)...",
        TEST_DURATION.as_secs(),
        TEST_DURATION.as_secs()
    );

    match test_download().await {
        Some(result) => tracing::info!(
            "Speed test DOWNLOAD — peak: {:.2} Mbps, stable avg: {:.2} Mbps",
            result.peak_mbps, result.stable_avg_mbps
        ),
        None => tracing::warn!("Speed test download failed or produced no samples"),
    }

    match test_upload().await {
        Some(result) => tracing::info!(
            "Speed test UPLOAD — peak: {:.2} Mbps, stable avg: {:.2} Mbps",
            result.peak_mbps, result.stable_avg_mbps
        ),
        None => tracing::warn!("Speed test upload failed or produced no samples"),
    }
}

fn bytes_to_mbps(bps: u64) -> f64 {
    bps as f64 * 8.0 / 1_000_000.0
}

async fn sample_throughput(
    counter: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    workers: Vec<tokio::task::JoinHandle<()>>,
) -> Option<SpeedResult> {
    let sample_count = (TEST_DURATION.as_millis() / SAMPLE_INTERVAL.as_millis()) as usize;
    let mut samples = Vec::with_capacity(sample_count);
    let mut last_bytes = 0u64;

    for _ in 0..sample_count {
        tokio::time::sleep(SAMPLE_INTERVAL).await;
        let current = counter.load(Ordering::Relaxed);
        let delta = current.saturating_sub(last_bytes);
        last_bytes = current;
        // delta is over SAMPLE_INTERVAL, convert to bytes/sec
        samples.push(delta * (1000 / SAMPLE_INTERVAL.as_millis() as u64));
    }

    stop.store(true, Ordering::Relaxed);
    for worker in workers {
        let _ = worker.await;
    }

    if samples.len() < RAMP_UP_SAMPLES + 5 {
        return None;
    }

    let peak_bps = samples.iter().copied().max().unwrap_or(0);
    let stable_samples = &samples[RAMP_UP_SAMPLES..];
    let stable_avg_bps = stable_samples.iter().sum::<u64>() / stable_samples.len() as u64;

    Some(SpeedResult {
        peak_mbps: bytes_to_mbps(peak_bps),
        stable_avg_mbps: bytes_to_mbps(stable_avg_bps),
    })
}

fn spawn_download_worker(
    client: reqwest::Client,
    counter: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
            match client.get(DOWNLOAD_URL).send().await {
                Ok(mut response) => {
                    // Stream and discard chunks instead of buffering the whole body.
                    while let Some(chunk) = response.chunk().await.ok().flatten() {
                        counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    // Brief pause to avoid tight error loop
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    })
}

async fn test_download() -> Option<SpeedResult> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;

    let counter = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    // Use two parallel download streams to saturate the link.
    let workers = vec![
        spawn_download_worker(client.clone(), counter.clone(), stop.clone()),
        spawn_download_worker(client, counter.clone(), stop.clone()),
    ];

    sample_throughput(counter, stop, workers).await
}

fn spawn_upload_worker(
    client: reqwest::Client,
    upload_data: Bytes,
    counter: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
            match client
                .post(UPLOAD_URL)
                .body(upload_data.clone())
                .send()
                .await
            {
                Ok(response) => {
                    // Count what we sent. Drop the response without reading the body
                    // to avoid extra allocations; HTTP status is enough to know it arrived.
                    if response.error_for_status().is_ok() {
                        counter.fetch_add(UPLOAD_CHUNK_BYTES as u64, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    })
}

async fn test_upload() -> Option<SpeedResult> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;

    // Allocate one reusable random buffer. Bytes::clone is cheap (Arc ref-count).
    let mut rng = rand::rng();
    let mut raw_data = vec![0u8; UPLOAD_CHUNK_BYTES];
    rng.fill(&mut raw_data[..]);
    let upload_data: Bytes = raw_data.into();

    let counter = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    let workers = vec![
        spawn_upload_worker(client.clone(), upload_data.clone(), counter.clone(), stop.clone()),
        spawn_upload_worker(client, upload_data, counter.clone(), stop.clone()),
    ];

    sample_throughput(counter, stop, workers).await
}

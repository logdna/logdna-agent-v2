use futures::future::{AbortHandle, Abortable};
use futures::stream;
use futures::stream::{Stream, StreamExt};
use hyper::{Client, StatusCode};
use prometheus_parse::{Sample, Scrape};
use std::time::Duration;

pub async fn fetch_agent_metrics(
    metrics_port: u16,
) -> Result<(StatusCode, Option<String>), hyper::Error> {
    let client = Client::new();
    let url = format!("http://127.0.0.1:{}/metrics", metrics_port)
        .parse()
        .unwrap();

    let resp = client.get(url).await?;
    let status = resp.status();
    let body = if status == StatusCode::OK {
        let buf = hyper::body::to_bytes(resp).await?;
        let body_str = std::str::from_utf8(&buf).unwrap().to_string();
        Some(body_str)
    } else {
        None
    };

    Ok((status, body))
}

fn stream_agent_metrics(
    metrics_port: u16,
    scrape_delay: Option<Duration>,
) -> impl Stream<Item = Sample> {
    stream::unfold(Vec::new(), move |state| async move {
        let mut state = loop {
            if !state.is_empty() {
                break state;
            }

            // Hitting the metrics endpoint too quick may result in subsequent queries without
            // any data points. Depending on need, one can specify a delay before scrapping giving
            // the agent time to generate some new metric data.
            if let Some(delay) = scrape_delay {
                tokio::time::sleep(delay).await;
            }

            match fetch_agent_metrics(metrics_port).await {
                Ok((StatusCode::OK, Some(body))) => {
                    let body = body.lines().map(|l| Ok(l.to_string()));
                    break Scrape::parse(body)
                        .expect("failed to parse the metrics response")
                        .samples;
                }
                Ok((status, _)) => panic!(
                    "The /metrics endpoint returned status code {:?}, expected 200.",
                    status
                ),
                Err(err) => panic!(
                    "Failed to make HTTP request to metrics endpoint - reason: {:?}",
                    err
                ),
            }
        };

        state.pop().map(|metric| (metric, state))
    })
}

pub struct MetricsRecorder {
    server: tokio::task::JoinHandle<Vec<Sample>>,
    abort_handle: AbortHandle,
}

impl MetricsRecorder {
    pub fn start(port: u16, scrape_delay: Option<Duration>) -> MetricsRecorder {
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        MetricsRecorder {
            server: tokio::spawn({
                async move {
                    Abortable::new(stream_agent_metrics(port, scrape_delay), abort_registration)
                        .collect::<Vec<Sample>>()
                        .await
                }
            }),
            abort_handle,
        }
    }

    pub async fn stop(self) -> Vec<Sample> {
        self.abort_handle.abort();
        match self.server.await {
            Ok(data) => data,
            Err(e) => panic!("error waiting for thread: {}", e),
        }
    }
}

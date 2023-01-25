use std::io::BufReader;
use std::time::Duration;

use float_cmp::approx_eq;
use prometheus_parse::{Sample, Value};
use tempfile::tempdir;

use crate::common::{self, start_ingester, AgentSettings};
use logdna_metrics_recorder::*;

use test_log::test;

fn check_fs_bytes(samples: &[Sample]) {
    let fs_bytes = samples
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if s.metric.as_str() == "logdna_agent_fs_bytes" => Some(raw),
            _ => None,
        })
        .collect::<Vec<f64>>();

    assert!(!fs_bytes.is_empty(), "no fs_byte metrics were captured");

    assert!(
        *fs_bytes.iter().last().unwrap() > f64::EPSILON,
        "last fs_byte datum should be greater than zero"
    );

    assert!(
        fs_bytes.windows(2).all(|w| match w {
            [a, b] => a <= b,
            _ => false,
        }),
        "fs_bytes should increase in count while run"
    );
}

fn check_fs_lines(samples: &[Sample]) {
    let fs_lines = samples
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if s.metric.as_str() == "logdna_agent_fs_lines" => Some(raw),
            _ => None,
        })
        .collect::<Vec<f64>>();

    assert!(!fs_lines.is_empty(), "no fs_line metrics were captured");

    assert!(
        *fs_lines.iter().last().unwrap() > f64::EPSILON,
        "last fs_line datum should be greater than zero "
    );

    assert!(
        fs_lines.windows(2).all(|w| match w {
            [a, b] => a <= b,
            _ => false,
        }),
        "fs_lines should increase in count while run"
    );
}

fn check_fs_events(samples: &[Sample]) {
    // logdna_agent_fs_events
    let fs_events = samples
        .iter()
        .filter_map(|s| match s.value {
            Value::Counter(raw) if s.metric == "logdna_agent_fs_events" => {
                Some((raw, s.labels.get("event_type")))
            }
            _ => None,
        })
        .collect::<Vec<(f64, Option<&str>)>>();

    assert!(!fs_events.is_empty(), "no fs_events metrics were captured");

    assert!(
        fs_events.iter().any(|result| match result {
            &(value, Some(event_type)) => value == 1.0 && event_type == "create",
            _ => false,
        }),
        "At least one file create event should be logged. Actual events: {:?}",
        fs_events
    );

    assert!(
        fs_events.iter().any(|result| match result {
            &(value, Some(event_type)) => value > 1.0 && event_type == "write",
            _ => false,
        }),
        "At least one file write event should be logged. Actual events: {:?}",
        fs_events
    );

    assert!(
        fs_events.iter().any(|result| match result {
            &(value, Some(event_type)) => value == 1.0 && event_type == "delete",
            _ => false,
        }),
        "At least one file delete event should be logged. Actual events: {:?}",
        fs_events
    );
}

fn check_fs_files(samples: &[Sample]) {
    // logdna_agent_fs_files
    let fs_files = samples
        .iter()
        .filter_map(|s| match s.value {
            Value::Gauge(raw) if s.metric.as_str() == "logdna_agent_fs_files" => Some(raw),
            _ => None,
        })
        .collect::<Vec<f64>>();

    assert!(!fs_files.is_empty(), "no fs_files metrics were captured");

    let first_fs_file = *fs_files.first().unwrap();
    let last_fs_file = *fs_files.iter().last().unwrap();
    assert!((first_fs_file as i64 - last_fs_file as i64).abs() <= 1);
    assert!(
        fs_files.iter().any(|v| *v >= first_fs_file),
        "at least one fs_files gauge should be above starting value"
    );
    let fs_max = fs_files.iter().fold(0f64, |a, x| a + x);
    assert!(
        fs_files.iter().any(|v| *v < fs_max),
        "at least one fs_files gauge should be lower than max"
    );
}

fn check_ingest_req_size(samples: &[Sample]) {
    let expected_bucket_boundaries = vec![
        500.0,
        1000.0,
        2000.0,
        4000.0,
        8000.0,
        16000.0,
        32000.0,
        64000.0,
        128000.0,
        256000.0,
        512000.0,
        1024000.0,
        2048000.0,
        f64::INFINITY,
    ];

    assert!(
        samples
            .iter()
            .filter_map(|s| match &s.value {
                Value::Histogram(counts)
                    if s.metric.as_str() == "logdna_agent_ingest_request_size_bucket" =>
                {
                    Some(counts.iter().map(|hc| hc.less_than).collect::<Vec<f64>>())
                }
                _ => None,
            })
            .all(|buckets| buckets == expected_bucket_boundaries),
        "logdna_agent_ingest_request_size bucket boundries do not match"
    );

    assert!(samples
        .iter()
        .filter_map(|s| match &s.value {
            Value::Histogram(counts)
                if s.metric.as_str() == "logdna_agent_ingest_request_size_bucket" =>
            {
                Some(counts.iter().map(|hc| hc.count).collect::<Vec<f64>>())
            }
            _ => None,
        })
        .any(|count| count.iter().any(|value| *value > 0.0)));
}

fn data_pair_for(name: &str) -> impl Fn(&Sample) -> Option<(i64, f64)> + '_ {
    move |s: &Sample| match (&s.value, &s.labels.get("outcome")) {
        (Value::Untyped(raw), Some("success")) if s.metric.as_str() == name => {
            Some((s.timestamp.timestamp_millis(), *raw))
        }
        _ => None,
    }
}

fn check_ingest_req_duration(samples: &[Sample]) {
    let counts = samples
        .iter()
        .filter_map(data_pair_for(
            "logdna_agent_ingest_request_duration_millis_count",
        ))
        .zip(samples.iter().filter_map(data_pair_for(
            "logdna_agent_ingest_request_duration_seconds_count",
        )))
        .collect::<Vec<((i64, f64), (i64, f64))>>();

    assert!(
        !counts.is_empty(),
        "should contain at least one duration count"
    );

    assert!(
        counts
            .iter()
            .all(|((ms_ts, ms_count), (sec_ts, sec_count))| {
                *ms_ts == *sec_ts && approx_eq!(f64, *ms_count, *sec_count)
            }),
        "counts for req/ms and req/sec should match"
    );

    let sums = samples
        .iter()
        .filter_map(data_pair_for(
            "logdna_agent_ingest_request_duration_millis_sum",
        ))
        .zip(samples.iter().filter_map(data_pair_for(
            "logdna_agent_ingest_request_duration_seconds_sum",
        )))
        .collect::<Vec<((i64, f64), (i64, f64))>>();

    assert!(
        sums.len() == counts.len(),
        "should have equal number of count and sum metrics"
    );

    assert!(
        sums.iter().all(|((ms_ts, ms_sum), (sec_ts, sec_sum))| {
            // There is some loss of precision in these metrics since the metric is a floating
            // point number and there is multiple conversions taking place before this point.
            // Loss of 1/10 of a millisecond seems acceptable to avoid a flakey test.
            *ms_ts == *sec_ts && approx_eq!(f64, *ms_sum, *sec_sum * 1000.0, epsilon = 0.1)
        }),
        "sums for req/ms and req/sec should be equal after conversion to ms; sums = {:?}",
        sums
    );
}

#[test(tokio::test)]
#[cfg_attr(not(feature = "integration_tests"), ignore)]
async fn test_metrics_endpoint() {
    let dir = tempdir().expect("Could not create temp dir").into_path();
    let included_file = dir.join("file1.log");
    let metrics_port = 9881;

    let (server, _, shutdown_ingest, address) =
        start_ingester(Box::new(|_| None), Box::new(|_| None));

    let mut settings = AgentSettings::with_mock_ingester(dir.to_str().unwrap(), &address);
    settings.metrics_port = Some(metrics_port);

    let mut agent_handle = common::spawn_agent(settings);
    let mut stderr_reader = BufReader::new(agent_handle.stderr.take().unwrap());
    common::wait_for_event("Enabling prometheus endpoint", &mut stderr_reader);

    let recorder = MetricsRecorder::start(metrics_port, Some(Duration::from_millis(100)));
    let (_, metrics_result) = tokio::join!(server, async move {
        // Wait for the agent to bootstrap and then start generating some log data
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Generate some log data with pauses to simulate some logging traffic
        for _ in 1..10 {
            common::append_to_file(&included_file, 100, 5).unwrap();
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }

        tokio::fs::remove_file(&included_file).await.unwrap();
        common::wait_for_event("Delete Event", &mut stderr_reader);
        common::consume_output(stderr_reader.into_inner());

        // Give the agent time to register the delete event before terminating
        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        shutdown_ingest();
        recorder.stop().await
    });

    common::assert_agent_running(&mut agent_handle);
    agent_handle.kill().expect("Could not kill process");

    check_fs_bytes(&metrics_result);
    check_fs_lines(&metrics_result);
    check_fs_events(&metrics_result);
    check_fs_files(&metrics_result);
    check_ingest_req_size(&metrics_result);
    check_ingest_req_duration(&metrics_result);
}

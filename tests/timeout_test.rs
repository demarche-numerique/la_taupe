mod helpers;
pub use helpers::*;
use la_taupe::http::analyze::AnalysisError;
use reqwest::blocking::Client;
use serde_json::json;
use std::time::Duration;

#[test]
fn analysis_timeout() {
    // Start a dedicated server with 1 second timeout for this test
    let _taupe_and_node = TaupeAndNode::start_with_timeout(Some(1));

    // Create a client with a longer timeout than the analysis timeout
    // so we can receive the 504 response
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    let response = client
        .post("http://localhost:8080/analyze")
        .json(&json!({
            "url": "http://localhost:3333/slow_analysis.png",
            "hint": { "type": "rib" },
        }))
        .send()
        .unwrap();

    // Should return 504 Gateway Timeout
    assert_eq!(response.status().as_u16(), 504);

    let analysis: AnalysisError = response.json().unwrap();

    let error_message = analysis.body.unwrap();
    assert!(
        error_message.contains("Analysis timeout: processing took more than 1 seconds"),
        "Expected timeout message, got: {}",
        error_message
    );
}

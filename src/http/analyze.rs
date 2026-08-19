use actix_web::{http::header::ContentType, post, web, HttpResponse, Responder};
use reqwest::Response;
use serde::{Deserialize, Serialize};

use crate::analysis::{Analysis, Hint};

const MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

#[derive(Deserialize)]
struct RequestedFile {
    url: String,
    hint: Option<Hint>,
}

#[derive(Deserialize, Serialize)]
pub struct AnalysisError {
    pub upstream_body: Option<String>,
    pub upstream_status_code: Option<u16>,
    pub body: Option<String>,
}

#[post("/analyze")]
pub async fn analyze(requested_file: web::Json<RequestedFile>) -> impl Responder {
    let response = match reqwest::get(&requested_file.url).await {
        Ok(response) => response,
        Err(e) => {
            log::error!("Request failed: {}", e);
            return HttpResponse::InternalServerError().json(AnalysisError {
                upstream_body: None,
                upstream_status_code: None,
                body: Some(format!("Request failed: {}", e)),
            });
        }
    };

    if response.status().is_success() {
        handle_response(response, requested_file.hint).await
    } else {
        handle_error(response).await
    }
}

async fn handle_error(resp: Response) -> HttpResponse {
    let status = resp.status();
    let upstream_status_code = Some(status.as_u16());
    let is_server_error = status.is_server_error();

    let upstream_body = Some(
        resp.text()
            .await
            .unwrap_or_else(|_| "unreadable upstream error".to_string()),
    );

    if is_server_error {
        let error = AnalysisError {
            upstream_body,
            upstream_status_code,
            body: Some("upstream server error".to_string()),
        };

        HttpResponse::BadGateway().json(error)
    } else {
        let error = AnalysisError {
            upstream_body,
            upstream_status_code,
            body: Some("upstream client error".to_string()),
        };

        HttpResponse::InternalServerError().json(error)
    }
}

async fn handle_response(mut resp: Response, hint: Option<Hint>) -> HttpResponse {
    let len = resp
        .headers()
        .get("content-length")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(MAX_FILE_SIZE);

    if len > MAX_FILE_SIZE {
        return HttpResponse::UnprocessableEntity()
            .content_type(ContentType::json())
            .json(AnalysisError {
                upstream_status_code: None,
                upstream_body: None,
                body: Some("File too big".to_string()),
            });
    }

    let mut bytes: Vec<u8> = Vec::with_capacity(len);

    while let Ok(chunk) = resp.chunk().await {
        match chunk {
            Some(data) => bytes.extend_from_slice(&data),
            None => break,
        }

        if bytes.len() > MAX_FILE_SIZE {
            break;
        }
    }

    let size = bytes.len();

    if size > MAX_FILE_SIZE {
        return HttpResponse::UnprocessableEntity()
            .content_type(ContentType::json())
            .json(AnalysisError {
                upstream_status_code: None,
                upstream_body: None,
                body: Some("File too big".to_string()),
            });
    }

    // L'analyse dure de quelques secondes à plusieurs dizaines : la laisser dans le
    // futur bloquerait un worker actix et toutes les connexions qu'il multiplexe.
    // web::block la porte sur le pool de threads bloquants.
    let outcome = web::block(move || Analysis::try_from((bytes, hint, "remote_file"))).await;

    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            log::error!("analysis task failed: {}", e);
            return HttpResponse::InternalServerError()
                .content_type(ContentType::json())
                .json(AnalysisError {
                    upstream_status_code: None,
                    upstream_body: None,
                    body: Some("analysis task failed".to_string()),
                });
        }
    };

    match outcome {
        Ok(analysis) => HttpResponse::Ok()
            .content_type(ContentType::json())
            .json(analysis),
        Err(error_msg) => HttpResponse::UnprocessableEntity()
            .content_type(ContentType::json())
            .json(AnalysisError {
                upstream_status_code: None,
                upstream_body: None,
                body: Some(error_msg),
            }),
    }
}

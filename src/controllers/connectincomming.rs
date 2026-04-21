#![allow(clippy::unused_async)]
use crate::{
    common,
    common::upload_tracker::{ActiveUploadGuard, UploadKind, UploadMeta, UploadTracker},
    workers::{
        bootlog_parser::{BootlogParserWorker, BootlogParserWorkerArgs},
        log_parser::{LogSegmentWorker, LogSegmentWorkerArgs},
    },
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use bytes::BytesMut;
use futures::StreamExt;
use loco_rs::prelude::*;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadAuthError {
    MissingDeviceAuth,
    UploadsDisabled,
    DongleMismatch,
}

fn validate_upload_auth(
    device_model: Option<&crate::models::devices::DM>,
    request_dongle_id: &str,
) -> std::result::Result<(), UploadAuthError> {
    let Some(device_model) = device_model else {
        return Err(UploadAuthError::MissingDeviceAuth);
    };

    if !device_model.uploads_allowed {
        return Err(UploadAuthError::UploadsDisabled);
    }

    if device_model.dongle_id != request_dongle_id {
        return Err(UploadAuthError::DongleMismatch);
    }

    Ok(())
}

fn enforce_upload_auth(
    auth: &crate::middleware::auth::MyJWT,
    request_dongle_id: &str,
) -> Result<()> {
    match validate_upload_auth(auth.device_model.as_ref(), request_dongle_id) {
        Ok(()) => Ok(()),
        Err(UploadAuthError::MissingDeviceAuth) => {
            loco_rs::controller::unauthorized("Only registered devices can upload")
        }
        Err(UploadAuthError::UploadsDisabled) => {
            loco_rs::controller::unauthorized("Uploads ignored")
        }
        Err(UploadAuthError::DongleMismatch) => {
            loco_rs::controller::bad_request("dongle_id does not match identity")
        }
    }
}

pub async fn upload_bootlogs(
    auth: crate::middleware::auth::MyJWT,
    Path((dongle_id, file)): Path<(String, String)>,
    State(ctx): State<AppContext>,
    axum::Extension(client): axum::Extension<reqwest::Client>,
    axum::Extension(upload_tracker): axum::Extension<std::sync::Arc<UploadTracker>>,
    body: axum::body::Body,
) -> Result<(StatusCode, &'static str)> {
    enforce_upload_auth(&auth, &dongle_id)?;
    let file_key = format!("{}_boot_{}", dongle_id, file);
    let _upload_guard = ActiveUploadGuard::new(
        upload_tracker.clone(),
        UploadMeta {
            kind: UploadKind::Bootlog,
            dongle_id: dongle_id.clone(),
            file_name: file_key.clone(),
        },
    );
    let upload_id = _upload_guard.id().to_string();

    let full_url = common::mkv_helpers::get_mkv_file_url(&file_key);

    let start = Instant::now();
    let mut buffer = BytesMut::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(data) => {
                upload_tracker.add_bytes(&upload_id, data.len());
                buffer.extend_from_slice(&data);
            }
            Err(e) => {
                tracing::warn!(
                    "Error reading request body after {} bytes in {:.2?}: {}",
                    buffer.len(),
                    start.elapsed(),
                    e
                );
                return Ok((StatusCode::BAD_REQUEST, "Error reading request body"));
            }
        }
    }
    drop(_upload_guard);
    let data = buffer.freeze();
    let data_len = data.len() as i64;
    tracing::info!("File `{}` received is {} bytes", full_url, data.len());

    // Post the binary data to the specified URL
    let response = client.put(&full_url).body(data).send().await;
    match response {
        Ok(response) => {
            let status = response.status();
            tracing::trace!("Got Ok response with status {status}");
            match status {
                StatusCode::FORBIDDEN => {
                    tracing::error!("Duplicate file uploaded");
                    return Ok((status, "Duplicate File Upload"));
                }
                StatusCode::CREATED | StatusCode::OK => {
                    if let Some(device) = auth.device_model {
                        let prev_server_usage = device.server_storage;
                        let mut active_device = device.into_active_model();
                        active_device.server_storage =
                            ActiveValue::Set(data_len + prev_server_usage);
                        match active_device.update(&ctx.db).await {
                            Ok(_) => (),
                            Err(e) => {
                                tracing::error!(
                                    "Failed to update active route model. DB Error {}",
                                    e.to_string()
                                );
                            }
                        }
                    }
                    // Enqueue the file for processing
                    tracing::debug!("File Uploaded Successfully. Queuing worker for {full_url}");
                    let result = BootlogParserWorker::perform_later(
                        &ctx,
                        BootlogParserWorkerArgs {
                            internal_file_url: full_url.clone(),
                            dongle_id: dongle_id,
                            file_name: file,
                            create_time: SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64,
                        },
                    )
                    .await;
                    match result {
                        // sorry this is kinda confusing
                        Ok(_) => {
                            tracing::debug!("Queued Worker");
                            return Ok((status, "Queued Worker"));
                        }
                        Err(e) => {
                            tracing::error!("Failed to queue worker: {}", format!("{}", e));
                            return Ok((
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "Failed to queue worker.",
                            ));
                        }
                    }
                }
                _ => {
                    tracing::error!("Unhandled status {}. File not uploaded.", status);
                    return Ok((status, "Unhandled status. File not uploaded."));
                }
            }
        }
        Err(e) => {
            tracing::error!("PUT request failed: {}", format!("{}", e));
            return Ok((StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong"));
        }
    }
}

pub async fn upload_crash(
    auth: crate::middleware::auth::MyJWT,
    Path((dongle_id, id, commit, name)): Path<(String, String, String, String)>, //:dongle_id/crash/:log_id/:commit/:name
    State(ctx): State<AppContext>,
    axum::Extension(client): axum::Extension<reqwest::Client>,
    axum::Extension(upload_tracker): axum::Extension<std::sync::Arc<UploadTracker>>,
    body: axum::body::Body,
) -> Result<(StatusCode, &'static str)> {
    enforce_upload_auth(&auth, &dongle_id)?;
    let file_key = format!("{}_crash_{}_{}_{}", dongle_id, id, commit, name);
    let _upload_guard = ActiveUploadGuard::new(
        upload_tracker.clone(),
        UploadMeta {
            kind: UploadKind::Crash,
            dongle_id: dongle_id.clone(),
            file_name: file_key.clone(),
        },
    );
    let upload_id = _upload_guard.id().to_string();

    let full_url = common::mkv_helpers::get_mkv_file_url(&file_key);

    let start = Instant::now();
    let mut buffer = BytesMut::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(data) => {
                upload_tracker.add_bytes(&upload_id, data.len());
                buffer.extend_from_slice(&data);
            }
            Err(e) => {
                tracing::warn!(
                    "Error reading request body after {} bytes in {:.2?}: {}",
                    buffer.len(),
                    start.elapsed(),
                    e
                );
                return Ok((StatusCode::BAD_REQUEST, "Error reading request body"));
            }
        }
    }
    drop(_upload_guard);
    let data = buffer.freeze();
    let data_len = data.len() as i64;
    tracing::info!("File `{}` received is {} bytes", full_url, data.len());

    // Post the binary data to the specified URL
    let response = client.put(&full_url).body(data).send().await;
    match response {
        Ok(response) => {
            let status = response.status();
            tracing::trace!("Got Ok response with status {status}");
            match status {
                StatusCode::FORBIDDEN => {
                    tracing::error!("Duplicate file uploaded");
                    return Ok((status, "Duplicate File Upload"));
                }
                StatusCode::CREATED | StatusCode::OK => {
                    // Enqueue the file for processing
                    tracing::debug!("{full_url} file Uploaded Successfully");
                    if let Some(device) = auth.device_model {
                        let prev_server_usage = device.server_storage;
                        let mut active_device = device.into_active_model();
                        active_device.server_storage =
                            ActiveValue::Set(data_len + prev_server_usage);
                        match active_device.update(&ctx.db).await {
                            Ok(_) => (),
                            Err(e) => {
                                tracing::error!(
                                    "Failed to update active route model. DB Error {}",
                                    e.to_string()
                                );
                            }
                        }
                    }
                    return Ok((status, "File uploaded."));
                }
                _ => {
                    tracing::error!("Unhandled status. File not uploaded.");
                    return Ok((status, "Unhandled status. File not uploaded."));
                }
            }
        }
        Err(e) => {
            tracing::error!("PUT request failed: {}", format!("{}", e));
            return Ok((StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong"));
        }
    }
}

pub async fn upload_driving_logs(
    auth: crate::middleware::auth::MyJWT,
    Path((dongle_id, timestamp, segment, file)): Path<(String, String, String, String)>,
    State(ctx): State<AppContext>,
    axum::Extension(client): axum::Extension<reqwest::Client>,
    axum::Extension(upload_tracker): axum::Extension<std::sync::Arc<UploadTracker>>,
    body: axum::body::Body,
) -> Result<(StatusCode, &'static str)> {
    let start = Instant::now();
    enforce_upload_auth(&auth, &dongle_id)?;
    // Construct the URL to store the file
    let file_key = format!("{}_{}--{}--{}", dongle_id, timestamp, segment, file);
    let _upload_guard = ActiveUploadGuard::new(
        upload_tracker.clone(),
        UploadMeta {
            kind: UploadKind::DrivingLog,
            dongle_id: dongle_id.clone(),
            file_name: file_key.clone(),
        },
    );
    let upload_id = _upload_guard.id().to_string();

    let full_url = common::mkv_helpers::get_mkv_file_url(&file_key);
    tracing::trace!("full_url: {full_url}");
    // Check for duplicate file
    //let response = client.request(&full_url).send().await;

    // Collect the binary data from the body
    let mut buffer = BytesMut::new();
    let mut stream = body.into_data_stream();

    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(data) => {
                upload_tracker.add_bytes(&upload_id, data.len());
                buffer.extend_from_slice(&data);
            }
            Err(e) => {
                tracing::warn!(
                    "Error reading request body after {} bytes in {:.2?}: {}",
                    buffer.len(),
                    start.elapsed(),
                    e
                );
                return Ok((StatusCode::BAD_REQUEST, "Error reading request body"));
            }
        }
    }
    drop(_upload_guard);
    
    let data = buffer.freeze();
    let data_len = data.len() as i64;

    let duration = start.elapsed();
    let secs = duration.as_secs_f64();
    let bytes_per_sec = data_len as f64 / secs;
    let mb_per_sec = bytes_per_sec / (1024.0 * 1024.0);

    tracing::info!(
        "File {} downloaded in {} bytes in {:.2?} seconds ({:.2} MB/s)",
        full_url,
        data_len,
        duration,
        mb_per_sec
    );

    // Post the binary data to the specified URL
    let response = client.put(&full_url).body(data).send().await;

    match response {
        Ok(response) => {
            let status = response.status();
            tracing::trace!("Got Ok response with status {status}");
            match status {
                StatusCode::FORBIDDEN => {
                    tracing::error!("Duplicate file uploaded");
                }
                StatusCode::CREATED | StatusCode::OK => {
                    if let Some(device) = auth.device_model {
                        let prev_server_usage = device.server_storage;
                        let mut active_device = device.into_active_model();
                        active_device.server_storage =
                            ActiveValue::Set(data_len + prev_server_usage);
                        match active_device.update(&ctx.db).await {
                            Ok(_) => (),
                            Err(e) => {
                                tracing::error!(
                                    "Failed to update active route model. DB Error {}",
                                    e.to_string()
                                );
                            }
                        }
                    }
                }
                _ => {
                    tracing::error!("Unhandled status. File not uploaded.");
                    return Ok((status, "Unhandled status. File not uploaded."));
                }
            }
            // Enqueue the file for processing
            tracing::debug!("File Uploaded Successfully. Queuing worker for {full_url}");
            let result = LogSegmentWorker::perform_later(
                &ctx,
                LogSegmentWorkerArgs {
                    internal_file_url: full_url,
                    dongle_id: dongle_id,
                    timestamp: timestamp,
                    segment: segment,
                    file: file,
                    create_time: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64,
                },
            )
            .await;
            match result {
                Ok(_) => {
                    tracing::debug!("Queued Worker");
                    return Ok((status, "Queued Worker"));
                }
                Err(e) => {
                    tracing::error!("Failed to queue worker: {}", format!("{}", e));
                    return Ok((StatusCode::INTERNAL_SERVER_ERROR, "Failed to queue worker."));
                }
            }
        }
        Err(e) => {
            tracing::error!("PUT request failed: {}", format!("{}", e));
            return Ok((StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong"));
        }
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("connectincoming")
        .add(
            "/:dongle_id/:timestamp/:segment/:file",
            put(upload_driving_logs),
        )
        .add("/:dongle_id/crash/:log_id/:commit/:name", put(upload_crash))
        .add("/:dongle_id/boot/:file", put(upload_bootlogs))
}

#[cfg(test)]
mod tests {
    use super::{validate_upload_auth, UploadAuthError};
    use crate::models::devices::DM;

    fn device_model(dongle_id: &str, uploads_allowed: bool) -> DM {
        DM {
            dongle_id: dongle_id.to_string(),
            uploads_allowed,
            ..Default::default()
        }
    }

    #[test]
    fn rejects_missing_device_auth() {
        let result = validate_upload_auth(None, "abc123");
        assert_eq!(result, Err(UploadAuthError::MissingDeviceAuth));
    }

    #[test]
    fn rejects_uploads_when_device_is_blocked() {
        let device = device_model("abc123", false);
        let result = validate_upload_auth(Some(&device), "abc123");
        assert_eq!(result, Err(UploadAuthError::UploadsDisabled));
    }

    #[test]
    fn rejects_cross_device_uploads() {
        let device = device_model("abc123", true);
        let result = validate_upload_auth(Some(&device), "other_device");
        assert_eq!(result, Err(UploadAuthError::DongleMismatch));
    }

    #[test]
    fn allows_matching_device_uploads() {
        let device = device_model("abc123", true);
        let result = validate_upload_auth(Some(&device), "abc123");
        assert_eq!(result, Ok(()));
    }
}

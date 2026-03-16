use matrix_sdk_crypto::CrossSigningBootstrapRequests;
use matrix_sdk_crypto::types::requests::AnyIncomingResponse;
use tracing::{info, warn};

use super::CryptoManager;

/// Maximum number of retry attempts for each bootstrap HTTP call.
const MAX_RETRIES: u32 = 3;

/// Check if an error is likely transient and worth retrying.
fn is_retryable(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    // Network / timeout / 5xx errors are retryable.
    msg.contains("timed out")
        || msg.contains("connection")
        || msg.contains("(500)")
        || msg.contains("(502)")
        || msg.contains("(503)")
        || msg.contains("(504)")
        || msg.contains("dns error")
}

impl CryptoManager {
    /// Bootstrap cross-signing keys (master, self-signing, user-signing).
    ///
    /// Generates the cross-signing identity, uploads the signing keys to the
    /// homeserver, and uploads device key signatures so that the device is
    /// verified by its owner's self-signing key.
    ///
    /// If cross-signing is already set up (`reset = false`), re-uploads the
    /// existing identity and re-signs the current device.
    ///
    /// Each HTTP step is retried up to [`MAX_RETRIES`] times with exponential
    /// backoff on transient errors (network, timeout, 5xx).
    pub async fn bootstrap_cross_signing(&self, reset: bool) -> anyhow::Result<()> {
        let status = self.machine.cross_signing_status().await;
        let has_master = status.has_master;
        let has_self_signing = status.has_self_signing;
        let has_user_signing = status.has_user_signing;

        info!(
            has_master,
            has_self_signing, has_user_signing, reset, "cross-signing status before bootstrap"
        );

        let CrossSigningBootstrapRequests {
            upload_keys_req,
            upload_signing_keys_req,
            upload_signatures_req,
        } = self.machine.bootstrap_cross_signing(reset).await?;

        // Step 1: Upload device keys if needed (fresh account).
        if let Some(req) = upload_keys_req {
            Self::with_retry("upload device keys", || async {
                self.dispatch_outgoing_request(req.request_id(), req.request())
                    .await
            })
            .await?;
            info!("cross-signing: device keys uploaded");
        }

        // Step 2: Upload cross-signing public keys.
        {
            let master_json = upload_signing_keys_req
                .master_key
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            let self_signing_json = upload_signing_keys_req
                .self_signing_key
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            let user_signing_json = upload_signing_keys_req
                .user_signing_key
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;

            Self::with_retry("upload signing keys", || async {
                self.matrix_client
                    .upload_signing_keys(
                        master_json.as_ref(),
                        self_signing_json.as_ref(),
                        user_signing_json.as_ref(),
                    )
                    .await
            })
            .await?;

            let resp = ruma::api::client::keys::upload_signing_keys::v3::Response::new();
            self.machine
                .mark_request_as_sent(
                    &ruma::TransactionId::new(),
                    AnyIncomingResponse::SigningKeysUpload(&resp),
                )
                .await?;

            info!("cross-signing: signing keys uploaded");
        }

        // Step 3: Upload signatures (self-signing key signs device keys).
        {
            let signed_keys_json = serde_json::to_value(&upload_signatures_req.signed_keys)?;

            Self::with_retry("upload signatures", || async {
                self.matrix_client
                    .upload_signatures(&signed_keys_json)
                    .await
            })
            .await?;

            let resp = ruma::api::client::keys::upload_signatures::v3::Response::new();
            self.machine
                .mark_request_as_sent(&ruma::TransactionId::new(), &resp)
                .await?;

            info!("cross-signing: device signatures uploaded");
        }

        info!(
            user_id = %self.machine.user_id(),
            device_id = %self.machine.device_id(),
            "cross-signing bootstrap complete"
        );

        Ok(())
    }

    /// Retry an async operation with exponential backoff on transient errors.
    async fn with_retry<F, Fut>(operation: &str, f: F) -> anyhow::Result<()>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            match f().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if attempt + 1 < MAX_RETRIES && is_retryable(&e) {
                        let delay = std::time::Duration::from_secs(1 << attempt);
                        warn!(
                            operation,
                            attempt = attempt + 1,
                            max = MAX_RETRIES,
                            delay_secs = delay.as_secs(),
                            error = %e,
                            "retrying after transient error"
                        );
                        tokio::time::sleep(delay).await;
                        last_err = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("{operation} failed after retries")))
    }
}

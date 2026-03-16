use std::collections::BTreeMap;

use matrix_sdk_crypto::types::requests::{AnyOutgoingRequest, KeysQueryRequest};
use ruma::{OwnedUserId, UserId};
use tracing::{debug, info};

use super::CryptoManager;

impl CryptoManager {
    /// Check whether our device keys are present on the homeserver.
    pub(super) async fn device_keys_on_server(&self) -> anyhow::Result<bool> {
        let user_id = self.machine.user_id().to_owned();
        let req = KeysQueryRequest {
            device_keys: BTreeMap::from([(user_id.clone(), Vec::new())]),
            timeout: Some(std::time::Duration::from_secs(10)),
        };
        let resp = self.matrix_client.query_keys_raw(&req).await?;

        let has_device = resp
            .device_keys
            .get(&user_id)
            .is_some_and(|devices| !devices.is_empty());

        Ok(has_device)
    }

    /// Upload device keys and one-time keys to the homeserver.
    ///
    /// Must be called on first startup and periodically when OTKs run low.
    /// Also processes any pending outgoing requests (key queries, claims, to-device).
    pub async fn process_outgoing_requests(&self) -> anyhow::Result<()> {
        let outgoing = self.machine.outgoing_requests().await?;

        for request in outgoing {
            self.dispatch_outgoing_request(request.request_id(), request.request())
                .await?;
        }

        Ok(())
    }

    /// Dispatch a single outgoing request to the homeserver and mark it as sent.
    pub(super) async fn dispatch_outgoing_request(
        &self,
        request_id: &ruma::TransactionId,
        request: &AnyOutgoingRequest,
    ) -> anyhow::Result<()> {
        match request {
            AnyOutgoingRequest::KeysUpload(req) => {
                let otk_count = req.one_time_keys.len();
                let has_device_keys = req.device_keys.is_some();
                let resp = self.matrix_client.upload_keys_raw(req).await?;
                info!(
                    has_device_keys,
                    otk_count,
                    otk_counts = ?resp.one_time_key_counts,
                    "keys uploaded"
                );
                self.machine.mark_request_as_sent(request_id, &resp).await?;
            }
            AnyOutgoingRequest::KeysQuery(req) => {
                let queried_users: Vec<String> =
                    req.device_keys.keys().map(|u| u.to_string()).collect();
                info!(users = ?queried_users, "keys query: requesting device keys");
                let resp = self.matrix_client.query_keys_raw(req).await?;
                for (user_id, devices) in &resp.device_keys {
                    info!(
                        user_id = %user_id,
                        device_count = devices.len(),
                        device_ids = ?devices.keys().map(|d| d.to_string()).collect::<Vec<_>>(),
                        "keys query: got devices"
                    );
                }
                self.machine.mark_request_as_sent(request_id, &resp).await?;
            }
            AnyOutgoingRequest::KeysClaim(req) => {
                let claim_users: Vec<String> =
                    req.one_time_keys.keys().map(|u| u.to_string()).collect();
                info!(users = ?claim_users, "keys claim: claiming OTKs");
                let resp = self.matrix_client.claim_keys_raw(req).await?;
                for (user_id, devices) in &resp.one_time_keys {
                    info!(
                        user_id = %user_id,
                        device_count = devices.len(),
                        "keys claim: got OTKs"
                    );
                }
                self.machine.mark_request_as_sent(request_id, &resp).await?;
            }
            AnyOutgoingRequest::ToDeviceRequest(req) => {
                let recipient_count: usize =
                    req.messages.values().map(|devices| devices.len()).sum();
                debug!(
                    event_type = %req.event_type,
                    recipients = recipient_count,
                    "sending to-device event"
                );
                let resp = self.matrix_client.send_to_device_raw(req).await?;
                self.machine.mark_request_as_sent(request_id, &resp).await?;
            }
            AnyOutgoingRequest::SignatureUpload(req) => {
                let signed_keys_json = serde_json::to_value(&req.signed_keys)?;
                self.matrix_client
                    .upload_signatures(&signed_keys_json)
                    .await?;
                let resp = ruma::api::client::keys::upload_signatures::v3::Response::new();
                self.machine.mark_request_as_sent(request_id, &resp).await?;
                info!("cross-signing signatures uploaded via outgoing request");
            }
            _ => {
                debug!("unhandled outgoing request type");
            }
        }
        Ok(())
    }

    /// Claim missing Olm sessions for the given users.
    pub(super) async fn claim_missing_sessions(
        &self,
        user_ids: &[OwnedUserId],
    ) -> anyhow::Result<()> {
        let refs: Vec<&UserId> = user_ids.iter().map(|u| u.as_ref()).collect();
        let claim_req = self.machine.get_missing_sessions(refs.into_iter()).await?;

        if let Some((txn_id, req)) = claim_req {
            let claim_users: Vec<String> =
                req.one_time_keys.keys().map(|u| u.to_string()).collect();
            info!(users = ?claim_users, "claiming missing Olm sessions");
            let resp = self.matrix_client.claim_keys_raw(&req).await?;
            self.machine.mark_request_as_sent(&txn_id, &resp).await?;
            info!("claimed missing Olm sessions successfully");
        } else {
            debug!("no missing Olm sessions to claim");
        }

        Ok(())
    }

    /// Track users so their device keys are queried and kept up-to-date.
    ///
    /// Also claims any missing Olm sessions for the tracked users.
    pub async fn update_tracked_users(&self, user_ids: &[OwnedUserId]) -> anyhow::Result<()> {
        let _guard = self.lock.write().await;

        let refs: Vec<&UserId> = user_ids.iter().map(|u| u.as_ref()).collect();
        self.machine.update_tracked_users(refs).await?;

        self.process_outgoing_requests().await?;

        self.claim_missing_sessions(user_ids).await?;

        debug!(count = user_ids.len(), "tracked users updated");
        Ok(())
    }

    /// Query encryption status: cross-signing state and device keys from server.
    pub async fn crypto_status(&self) -> anyhow::Result<super::CryptoStatus> {
        let cross_signing = self.machine.cross_signing_status().await;

        let user_id = self.machine.user_id().to_owned();
        let device_id = self.machine.device_id().to_owned();
        let req = KeysQueryRequest {
            device_keys: BTreeMap::from([(user_id.clone(), Vec::new())]),
            timeout: Some(std::time::Duration::from_secs(10)),
        };
        let resp = self.matrix_client.query_keys_raw(&req).await?;

        let device_info = resp
            .device_keys
            .get(&user_id)
            .and_then(|devices| devices.get(&device_id))
            .map(|raw| serde_json::to_value(raw).unwrap_or_default());

        Ok(super::CryptoStatus {
            user_id: user_id.to_string(),
            device_id: device_id.to_string(),
            has_master_key: cross_signing.has_master,
            has_self_signing_key: cross_signing.has_self_signing,
            has_user_signing_key: cross_signing.has_user_signing,
            device_keys_uploaded: device_info.is_some(),
            device_keys: device_info,
        })
    }
}

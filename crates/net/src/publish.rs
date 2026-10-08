//! Requests to a tracker about its listings (D-128, D-129): signing in with this peer's key and
//! listing bundles. The API is `tempest_tracker_api`'s.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_protocol::BundleId;
use tempest_tracker_api::account::{Challenge, Session, SignIn, authority, sign_in_message};
use tempest_tracker_api::listing::{Listing, ListingEdit};

use crate::identity::Identity;
use crate::tracker::{TrackerClient, TrackerError};

impl TrackerClient {
    /// Signs in to the tracker at `tracker` with `identity`'s key, using `invite` if the tracker
    /// needs one for a first sign-in. The session lasts until it expires or
    /// [`sign_out`](Self::sign_out) ends it.
    pub async fn sign_in(
        &self,
        tracker: &str,
        identity: &Identity,
        invite: Option<&str>,
    ) -> Result<Session, TrackerError> {
        let challenge: Challenge = self.post_json(tracker, "session/challenges", &()).await?;
        self.sign_code(tracker, identity, &challenge.code, invite)
            .await
    }

    /// Signs the sign-in `code` another client asked for, such as a browser showing the
    /// tracker's website, with `identity`'s key. That client collects the session; this one gets it
    /// too.
    pub async fn sign_code(
        &self,
        tracker: &str,
        identity: &Identity,
        code: &str,
        invite: Option<&str>,
    ) -> Result<Session, TrackerError> {
        let authority = authority(tracker).ok_or(TrackerError::NotHttp)?;
        let code = tempest_tracker_api::account::normalize_code(code);
        let request = SignIn {
            sig: identity.sign(&sign_in_message(&authority, &code)),
            code,
            key: identity.peer_id(),
            invite: invite.map(str::to_owned),
        };
        self.post_json(tracker, "session", &request).await
    }

    /// Ends the session whose token is `token`.
    pub async fn sign_out(&self, tracker: &str, token: &str) -> Result<(), TrackerError> {
        let request = self
            .http
            .delete(tempest_protocol::tracker::endpoint_url(tracker, "session"))
            .bearer_auth(token);
        let _: serde_json::Value = self.exchange(request).await?;
        Ok(())
    }

    /// Lists `bundle` under the session's key with `edit`'s fields, or changes its listing.
    pub async fn edit_listing(
        &self,
        tracker: &str,
        token: &str,
        bundle: &BundleId,
        edit: &ListingEdit,
    ) -> Result<Listing, TrackerError> {
        let path = format!("listings/{bundle}");
        let request = self
            .http
            .put(tempest_protocol::tracker::endpoint_url(tracker, &path))
            .bearer_auth(token);
        self.exchange(json(request, edit)).await
    }

    /// Removes the session's key's listing of `bundle`.
    pub async fn unpublish_listing(
        &self,
        tracker: &str,
        token: &str,
        bundle: &BundleId,
    ) -> Result<(), TrackerError> {
        let path = format!("listings/{bundle}");
        let request = self
            .http
            .delete(tempest_protocol::tracker::endpoint_url(tracker, &path))
            .bearer_auth(token);
        let _: serde_json::Value = self.exchange(request).await?;
        Ok(())
    }

    /// `bundle`'s listing, as the session whose token is `token` sees it, if one is given.
    pub async fn listing(
        &self,
        tracker: &str,
        token: Option<&str>,
        bundle: &BundleId,
    ) -> Result<Listing, TrackerError> {
        let path = format!("listings/{bundle}");
        let mut request = self
            .http
            .get(tempest_protocol::tracker::endpoint_url(tracker, &path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        self.exchange(request).await
    }

    async fn post_json<T: DeserializeOwned>(
        &self,
        tracker: &str,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, TrackerError> {
        let request = self
            .http
            .post(tempest_protocol::tracker::endpoint_url(tracker, path));
        match self.exchange(json(request, body)).await {
            // Only a tracker without listings lacks the sign-in endpoints.
            Err(TrackerError::Refused { status: 404, .. }) => Err(TrackerError::NoListings),
            result => result,
        }
    }
}

/// `request` with `body` as its JSON body.
fn json(request: reqwest::RequestBuilder, body: &impl Serialize) -> reqwest::RequestBuilder {
    let body = serde_json::to_vec(body).expect("the request types serialize");
    request
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
}

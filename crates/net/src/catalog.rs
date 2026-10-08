//! Requests to a tracker's catalog (D-128, D-129): signing in with this peer's key and listing
//! bundles. The catalog's API is `tempest_catalog_api`'s.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tempest_catalog_api::account::{Challenge, Session, SignIn, authority, sign_in_message};
use tempest_catalog_api::listing::{Listing, ListingEdit};
use tempest_protocol::BundleId;

use crate::identity::Identity;
use crate::tracker::{TrackerClient, TrackerError};

impl TrackerClient {
    /// Signs in to the catalog of the tracker at `tracker` with `identity`'s key, using `invite`
    /// if the catalog needs one for a first sign-in. The session lasts until it expires or
    /// [`catalog_sign_out`](Self::catalog_sign_out) ends it.
    pub async fn catalog_sign_in(
        &self,
        tracker: &str,
        identity: &Identity,
        invite: Option<&str>,
    ) -> Result<Session, TrackerError> {
        let challenge: Challenge = self.catalog_post(tracker, "challenges", &()).await?;
        self.catalog_sign_code(tracker, identity, &challenge.code, invite)
            .await
    }

    /// Signs the sign-in `code` another client asked for, such as a browser showing the
    /// catalog, with `identity`'s key. That client collects the session; this one gets it too.
    pub async fn catalog_sign_code(
        &self,
        tracker: &str,
        identity: &Identity,
        code: &str,
        invite: Option<&str>,
    ) -> Result<Session, TrackerError> {
        let authority = authority(tracker).ok_or(TrackerError::NotHttp)?;
        let code = tempest_catalog_api::account::normalize_code(code);
        let request = SignIn {
            sig: identity.sign(&sign_in_message(&authority, &code)),
            code,
            key: identity.peer_id(),
            invite: invite.map(str::to_owned),
        };
        self.catalog_post(tracker, "sessions", &request).await
    }

    /// Ends the session whose token is `token`.
    pub async fn catalog_sign_out(&self, tracker: &str, token: &str) -> Result<(), TrackerError> {
        let request = self
            .http
            .delete(tempest_catalog_api::url(tracker, "session"))
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
            .put(tempest_catalog_api::url(tracker, &path))
            .bearer_auth(token);
        self.exchange(json(request, edit)).await
    }

    /// Withdraws the session's key's listing of `bundle`.
    pub async fn withdraw_listing(
        &self,
        tracker: &str,
        token: &str,
        bundle: &BundleId,
    ) -> Result<(), TrackerError> {
        let path = format!("listings/{bundle}");
        let request = self
            .http
            .delete(tempest_catalog_api::url(tracker, &path))
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
        let mut request = self.http.get(tempest_catalog_api::url(tracker, &path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        self.exchange(request).await
    }

    async fn catalog_post<T: DeserializeOwned>(
        &self,
        tracker: &str,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, TrackerError> {
        let request = self.http.post(tempest_catalog_api::url(tracker, path));
        self.exchange(json(request, body)).await
    }
}

/// `request` with `body` as its JSON body.
fn json(request: reqwest::RequestBuilder, body: &impl Serialize) -> reqwest::RequestBuilder {
    let body = serde_json::to_vec(body).expect("the catalog's request types serialize");
    request
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
}

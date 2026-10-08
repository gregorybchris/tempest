//! `tempest publish`, `unpublish` and `sign-in`: listing transfers on trackers and signing in to
//! their websites (D-128, D-129), with this installation's key, through the daemon if one is
//! running.

use anyhow::Result;
use tempest_engine::api::{PublishRequest, TrackerSignInRequest};
use tempest_tracker_api::listing::{Listing, ListingEdit, ListingStatus};

use crate::engine::{EngineArgs, Target};

/// `tempest sign-in`: signs a browser in to a tracker's website.
#[derive(Debug, clap::Args)]
pub struct SignInArgs {
    /// The tracker's URL, as the browser shows it, such as `https://tracker.example.org`.
    tracker: String,
    /// The sign-in code the tracker's website shows.
    #[arg(long)]
    code: String,
    /// An invite, for a first sign-in to a tracker that requires one.
    #[arg(long)]
    invite: Option<String>,
    #[command(flatten)]
    engine: EngineArgs,
}

/// `tempest publish`: lists a transfer on a tracker, or changes its listing.
#[derive(Debug, clap::Args)]
pub struct PublishArgs {
    /// The transfer: a bundle ID, a unique prefix of one, or a name.
    transfer: String,
    /// The tracker's URL, such as `https://tracker.example.org`.
    tracker: String,
    #[command(flatten)]
    listing: ListingArgs,
    #[command(flatten)]
    engine: EngineArgs,
}

/// `tempest unpublish`: removes a transfer's listing from a tracker.
#[derive(Debug, clap::Args)]
pub struct UnpublishArgs {
    /// The transfer: a bundle ID, a unique prefix of one, or a name.
    transfer: String,
    /// The tracker's URL, such as `https://tracker.example.org`.
    tracker: String,
    #[command(flatten)]
    engine: EngineArgs,
}

/// A listing's fields, for `tempest share --publish` and `tempest publish`.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct ListingArgs {
    /// The listing's title. Defaults to the bundle's name.
    #[arg(long)]
    pub title: Option<String>,
    /// The listing's description.
    #[arg(long)]
    pub description: Option<String>,
    /// The listing's category, by its slug, such as `datasets`.
    #[arg(long)]
    pub category: Option<String>,
    /// A tag for the listing. Repeat for more; replaces the listing's tags.
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// An invite, for a first sign-in to a tracker that requires one.
    #[arg(long)]
    pub invite: Option<String>,
}

impl ListingArgs {
    /// The fields given, leaving the others as the listing has them.
    pub fn edit(&self) -> ListingEdit {
        ListingEdit {
            title: self.title.clone(),
            description: self.description.clone(),
            category: self.category.clone().map(Some),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
        }
    }

    /// The request that publishes on `tracker`.
    pub fn request(&self, tracker: &str) -> PublishRequest {
        PublishRequest {
            tracker: tracker.to_owned(),
            listing: self.edit(),
            invite: self.invite.clone(),
        }
    }
}

/// Prints where a listing stands.
pub fn print_listing(tracker: &str, listing: &Listing) {
    let status = match listing.status {
        ListingStatus::Pending => {
            "pending: listed once the tracker has fetched the manifest from the swarm"
        }
        ListingStatus::Listed => "listed",
        ListingStatus::Unpublished => "unpublished",
    };
    println!("tracker   {} ({status})", tracker.trim_end_matches('/'));
}

pub fn sign_in(args: SignInArgs) -> Result<()> {
    let SignInArgs {
        tracker,
        code,
        invite,
        engine,
    } = args;
    let request = TrackerSignInRequest {
        tracker,
        code,
        invite,
    };
    let account = Target::run(&engine, async |target| {
        Ok(match target {
            Target::Daemon(client) => client.tracker_sign_in(&request).await?,
            Target::Held(engine) => {
                let invite = request.invite.as_deref();
                engine
                    .sign_in(&request.tracker, &request.code, invite)
                    .await?
            }
        })
    })?;
    eprintln!("signed in as {}", account.key);
    Ok(())
}

pub fn publish(args: PublishArgs) -> Result<()> {
    let PublishArgs {
        transfer,
        tracker,
        listing,
        engine,
    } = args;
    let request = listing.request(&tracker);
    let listing = Target::run(&engine, async |target| {
        let id = target.find(&transfer).await?.id;
        Ok(match target {
            Target::Daemon(client) => client.publish(id, &request).await?,
            Target::Held(engine) => {
                let invite = request.invite.as_deref();
                engine
                    .publish(id, &request.tracker, &request.listing, invite)
                    .await?
            }
        })
    })?;
    print_listing(&tracker, &listing);
    Ok(())
}

pub fn unpublish(args: UnpublishArgs) -> Result<()> {
    let UnpublishArgs {
        transfer,
        tracker,
        engine,
    } = args;
    Target::run(&engine, async |target| {
        let id = target.find(&transfer).await?.id;
        match target {
            Target::Daemon(client) => client.unpublish(id, &tracker).await?,
            Target::Held(engine) => engine.unpublish(id, &tracker).await?,
        };
        Ok(())
    })?;
    println!("tracker   {} (unpublished)", tracker.trim_end_matches('/'));
    Ok(())
}

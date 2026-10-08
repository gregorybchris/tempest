//! `tempest catalog`: signing in to trackers' catalogs and listing transfers there (D-128,
//! D-129), with this installation's key, through the daemon if one is running.

use anyhow::Result;
use tempest_catalog_api::listing::{Listing, ListingEdit, ListingStatus};
use tempest_engine::api::{CatalogSignInRequest, PublishRequest};

use crate::engine::{EngineArgs, Target};

#[derive(Debug, clap::Subcommand)]
pub enum CatalogCommand {
    /// Sign in to a tracker's catalog in a browser: sign the code the catalog shows with this
    /// installation's key.
    SignIn {
        /// The tracker's URL, as the browser shows it, such as `https://tracker.example.org`.
        tracker: String,
        /// The sign-in code the catalog shows.
        #[arg(long)]
        code: String,
        /// An invite, for a first sign-in to a catalog that requires one.
        #[arg(long)]
        invite: Option<String>,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// List a transfer on a tracker's catalog, or change its listing.
    Publish {
        /// The transfer: a bundle ID, a unique prefix of one, or a name.
        transfer: String,
        /// The tracker's URL, such as `https://tracker.example.org`.
        tracker: String,
        #[command(flatten)]
        listing: ListingArgs,
        #[command(flatten)]
        engine: EngineArgs,
    },
}

/// A listing's fields, for `tempest share --publish` and `tempest catalog publish`.
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
    /// An invite, for a first sign-in to a catalog that requires one.
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
        ListingStatus::Withdrawn => "withdrawn",
    };
    println!("catalog   {} ({status})", tracker.trim_end_matches('/'));
}

pub fn catalog(command: CatalogCommand) -> Result<()> {
    match command {
        CatalogCommand::SignIn {
            tracker,
            code,
            invite,
            engine,
        } => {
            let request = CatalogSignInRequest {
                tracker,
                code,
                invite,
            };
            let account = Target::run(&engine, async |target| {
                Ok(match target {
                    Target::Daemon(client) => client.catalog_sign_in(&request).await?,
                    Target::Held(engine) => {
                        let invite = request.invite.as_deref();
                        engine
                            .catalog_sign_in(&request.tracker, &request.code, invite)
                            .await?
                    }
                })
            })?;
            eprintln!("signed in as {}", account.key);
            Ok(())
        }
        CatalogCommand::Publish {
            transfer,
            tracker,
            listing,
            engine,
        } => {
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
    }
}

//! Where the page is, kept in the address's `#` so that every view has a link and the
//! browser's back button works: `#/?q=rain&category=datasets` (browsing, with the API's own
//! query string), `#/listings/{bundle}`, `#/listings/{bundle}/edit`, `#/sign-in` and `#/me`.

use tempest_protocol::BundleId;
use tempest_tracker_api::query::ListingQuery;

/// A view and what it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Listings, searched, filtered, sorted and paged.
    Browse(ListingQuery),
    /// One listing.
    Listing(BundleId),
    /// The publisher's form for a listing.
    Edit(BundleId),
    /// Signing in with a Tempest key.
    SignIn,
    /// The account signed in, and its listings.
    Me,
}

impl Default for Route {
    fn default() -> Self {
        Route::Browse(ListingQuery::default())
    }
}

impl Route {
    /// The route a `#` fragment names, with or without the `#`. Anything unknown or unreadable
    /// is the first page of listings.
    pub fn parse(hash: &str) -> Route {
        let hash = hash.strip_prefix('#').unwrap_or(hash);
        let (path, query) = hash.split_once('?').unwrap_or((hash, ""));
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let route = match parts[..] {
            [] => ListingQuery::parse(query).ok().map(Route::Browse),
            ["listings", bundle] => bundle.parse().ok().map(Route::Listing),
            ["listings", bundle, "edit"] => bundle.parse().ok().map(Route::Edit),
            ["sign-in"] => Some(Route::SignIn),
            ["me"] => Some(Route::Me),
            _ => None,
        };
        route.unwrap_or_default()
    }

    /// Its `#` fragment.
    pub fn hash(&self) -> String {
        match self {
            Route::Browse(query) => match query.to_query() {
                query if query.is_empty() => "#/".to_owned(),
                query => format!("#/?{query}"),
            },
            Route::Listing(bundle) => format!("#/listings/{bundle}"),
            Route::Edit(bundle) => format!("#/listings/{bundle}/edit"),
            Route::SignIn => "#/sign-in".to_owned(),
            Route::Me => "#/me".to_owned(),
        }
    }

    /// The view `esc` goes back to, if any.
    pub fn up(&self) -> Option<Route> {
        match self {
            Route::Browse(_) => None,
            Route::Edit(bundle) => Some(Route::Listing(*bundle)),
            Route::Listing(_) | Route::SignIn | Route::Me => Some(Route::default()),
        }
    }
}

impl tempest_ui_kit::shell::Route for Route {
    fn parse(hash: &str) -> Self {
        Route::parse(hash)
    }

    fn hash(&self) -> String {
        Route::hash(self)
    }
}

#[cfg(test)]
mod tests {
    use tempest_tracker_api::query::Sort;

    use super::*;

    const BUNDLE: &str = "3f9a000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn routes_read_what_they_write() {
        let bundle: BundleId = BUNDLE.parse().unwrap();
        let routes = [
            Route::default(),
            Route::Browse(ListingQuery {
                q: "rain fall & snow".to_owned(),
                category: Some("datasets".to_owned()),
                tags: vec!["weather".to_owned(), "csv".to_owned()],
                sort: Sort::Trending,
                offset: 50,
                ..ListingQuery::default()
            }),
            Route::Listing(bundle),
            Route::Edit(bundle),
            Route::SignIn,
            Route::Me,
        ];
        for route in routes {
            assert_eq!(Route::parse(&route.hash()), route, "{}", route.hash());
        }
        assert_eq!(Route::default().hash(), "#/");
        assert_eq!(
            Route::Listing(bundle).hash(),
            format!("#/listings/{BUNDLE}")
        );
    }

    #[test]
    fn anything_unknown_is_the_first_page() {
        for hash in [
            "",
            "#",
            "#/",
            "#/nowhere",
            "#/listings/xyz",
            "#/?sort=loudest",
            "#/?limit=0",
            "#/a/b/c/d",
        ] {
            assert_eq!(Route::parse(hash), Route::default(), "{hash}");
        }
        assert_eq!(
            Route::Edit(BUNDLE.parse().unwrap()).up(),
            Some(Route::Listing(BUNDLE.parse().unwrap()))
        );
        assert_eq!(Route::default().up(), None);
    }
}

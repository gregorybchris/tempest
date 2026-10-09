//! Tables: sortable headers, and rows that follow their link.

use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Whether a click on a table row whose first link goes where the row does should follow
/// the row: a plain click, not one on a link, which goes by itself, nor one with a modifier,
/// which opens the link elsewhere.
pub fn row_click(e: &ev::MouseEvent) -> bool {
    let modified = e.ctrl_key() || e.meta_key() || e.shift_key() || e.alt_key();
    let on_link = e
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|t| t.closest("a").ok().flatten().is_some());
    e.button() == 0 && !modified && !on_link
}

/// Which column a table is sorted by, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<K> {
    /// The column.
    pub key: K,
    /// Largest or last first.
    pub descending: bool,
}

impl<K: PartialEq> Sort<K> {
    /// The sort after a click on column `key`: the same column flips, and another starts in its
    /// natural direction, largest first for numbers and A to Z for text.
    pub fn clicked(self, key: K, numeric: bool) -> Sort<K> {
        if self.key == key {
            Sort {
                key,
                descending: !self.descending,
            }
        } else {
            Sort {
                key,
                descending: numeric,
            }
        }
    }
}

/// A table header that sorts by its column.
#[component]
pub fn SortHeader<K>(
    /// Its label.
    label: &'static str,
    /// Its column.
    key: K,
    /// The table's sort.
    sort: RwSignal<Sort<K>>,
    /// Whether the column holds numbers, which sort largest first and align right.
    #[prop(optional)]
    numeric: bool,
) -> impl IntoView
where
    K: Copy + PartialEq + Send + Sync + 'static,
{
    let direction = move || {
        sort.with(|s| {
            (s.key == key).then_some(if s.descending {
                "descending"
            } else {
                "ascending"
            })
        })
    };
    view! {
        <th class:num=numeric aria-sort=direction>
            <button class="sort" type="button" data-sort=direction on:click=move |_| sort.update(|s| *s = s.clicked(key, numeric))>
                {label}
            </button>
        </th>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorting_flips_the_same_column_and_starts_others_naturally() {
        let sort = Sort {
            key: "name",
            descending: false,
        };
        assert_eq!(
            sort.clicked("name", false),
            Sort {
                key: "name",
                descending: true
            }
        );
        assert_eq!(
            sort.clicked("size", true),
            Sort {
                key: "size",
                descending: true
            }
        );
        assert_eq!(
            sort.clicked("status", false),
            Sort {
                key: "status",
                descending: false
            }
        );
    }
}

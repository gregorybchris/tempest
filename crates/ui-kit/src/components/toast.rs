//! Toasts: short messages that dismiss themselves.

use std::time::Duration;

use leptos::prelude::*;

use super::Tone;

/// A short message that dismisses itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    id: u64,
    /// Its accent: [`Tone::Bad`] for errors.
    pub tone: Tone,
    /// What happened.
    pub text: String,
}

/// The page's toasts. [`Toaster`] provides it as context.
#[derive(Clone, Copy)]
pub struct Toasts {
    shown: RwSignal<Vec<Toast>>,
    next: StoredValue<u64>,
}

impl Toasts {
    /// How long a toast stays; errors stay twice as long.
    const SHOWN_FOR: Duration = Duration::from_secs(5);

    /// Shows `text`.
    pub fn push(self, tone: Tone, text: impl Into<String>) {
        let id = self.next.get_value();
        self.next.set_value(id + 1);
        self.shown.update(|shown| {
            shown.push(Toast {
                id,
                tone,
                text: text.into(),
            });
            // A burst of messages keeps only the latest few.
            if shown.len() > 4 {
                shown.remove(0);
            }
        });
        let stay = if tone == Tone::Bad {
            Self::SHOWN_FOR * 2
        } else {
            Self::SHOWN_FOR
        };
        set_timeout(move || self.dismiss(id), stay);
    }

    fn dismiss(self, id: u64) {
        // The page may have been torn down in the meantime.
        let _ = self.shown.try_update(|shown| shown.retain(|t| t.id != id));
    }
}

/// The page's [`Toasts`], from context.
///
/// # Panics
///
/// Outside a [`Toaster`].
pub fn use_toasts() -> Toasts {
    expect_context()
}

/// Provides [`Toasts`] to its children and shows them at the bottom right.
#[component]
pub fn Toaster(children: Children) -> impl IntoView {
    let toasts = Toasts {
        shown: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
    };
    provide_context(toasts);
    view! {
        {children()}
        <div class="toasts" role="status" aria-live="polite">
            <For each=move || toasts.shown.get() key=|t| t.id let:toast>
                <div class=format!("toast {}", toast.tone.class())>
                    <span class="toast__text">{toast.text}</span>
                    <button class="close" type="button" aria-label="Dismiss" on:click=move |_| toasts.dismiss(toast.id)>
                        "×"
                    </button>
                </div>
            </For>
        </div>
    }
}

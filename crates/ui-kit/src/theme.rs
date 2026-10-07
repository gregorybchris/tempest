//! Themes and effects: which palette the page wears, and whether it glows.

use leptos::prelude::*;

/// A palette (D-089). All four ship, and green is the default (D-135).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Theme {
    /// Green phosphor.
    #[default]
    Green,
    /// Amber phosphor.
    Amber,
    /// Neon vector, after the arcade *Tempest*.
    Vector,
    /// Beige-era computing, the light theme.
    Beige,
}

impl Theme {
    /// Every theme, in the order pickers show them.
    pub const ALL: [Theme; 4] = [Theme::Green, Theme::Amber, Theme::Vector, Theme::Beige];

    /// Its name in settings and in the stylesheet's `data-theme`.
    pub fn name(self) -> &'static str {
        match self {
            Theme::Green => "green",
            Theme::Amber => "amber",
            Theme::Vector => "vector",
            Theme::Beige => "beige",
        }
    }

    /// The theme called `name`, if there is one.
    pub fn named(name: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|theme| theme.name() == name)
    }

    /// The theme after this one, wrapping around.
    pub fn next(self) -> Theme {
        let i = Theme::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Theme::ALL[(i + 1) % Theme::ALL.len()]
    }
}

/// How the page looks: its theme and its CRT effects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    /// The palette.
    pub theme: Theme,
    /// Scanlines over the page, off by default.
    pub scanlines: bool,
    /// No glow, scanlines or animation, whatever else is chosen.
    pub clean: bool,
}

/// Keeps `<html>`'s `data-theme`, `data-scanlines` and `data-look` in step with `look`, which is
/// all the stylesheet needs.
pub fn apply_look(look: Signal<Look>) {
    Effect::new(move |_| {
        let look = look.get();
        let Some(root) = document().document_element() else {
            return;
        };
        let scanlines = if look.scanlines { "on" } else { "off" };
        let effects = if look.clean { "clean" } else { "crt" };
        for (name, value) in [
            ("data-theme", look.theme.name()),
            ("data-scanlines", scanlines),
            ("data-look", effects),
        ] {
            // Setting an attribute only fails for invalid names.
            let _ = root.set_attribute(name, value);
        }
    });
}

/// Buttons to pick a theme.
#[component]
pub fn ThemePicker(
    /// The theme shown, which the buttons set.
    theme: RwSignal<Theme>,
) -> impl IntoView {
    view! {
        <div class="segmented" role="group" aria-label="Theme">
            {Theme::ALL
                .map(|option| {
                    view! {
                        <button
                            class="segmented__option"
                            type="button"
                            data-theme-option=option.name()
                            aria-pressed=move || bool_attr(theme.get() == option)
                            on:click=move |_| theme.set(option)
                        >
                            {option.name()}
                        </button>
                    }
                })}
        </div>
    }
}

/// `"true"` or `"false"`, for ARIA attributes that need the words.
pub fn bool_attr(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_next_cycles() {
        for theme in Theme::ALL {
            assert_eq!(Theme::named(theme.name()), Some(theme));
        }
        assert_eq!(Theme::named("mauve"), None);
        assert_eq!(Theme::default(), Theme::Green);
        let mut theme = Theme::Green;
        for _ in 0..Theme::ALL.len() {
            theme = theme.next();
        }
        assert_eq!(theme, Theme::Green);
    }
}

//! Buttons, `[x] label` toggles and rows of exclusive options.

use leptos::prelude::*;

use crate::theme::bool_attr;

/// How a button looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonKind {
    /// A framed button.
    #[default]
    Plain,
    /// The main action on a screen.
    Primary,
    /// An action that destroys something.
    Danger,
    /// A frameless button for minor actions.
    Ghost,
}

/// A button.
#[component]
pub fn Button(
    /// What happens on a click.
    #[prop(into)]
    on_click: Callback<()>,
    /// How it looks.
    #[prop(default = ButtonKind::Plain)]
    kind: ButtonKind,
    /// A smaller size, for rows and toolbars.
    #[prop(optional)]
    small: bool,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// A tooltip, such as its keyboard shortcut.
    #[prop(optional, into)]
    title: MaybeProp<String>,
    /// Its label.
    children: Children,
) -> impl IntoView {
    let mut class = "btn".to_owned();
    class.push_str(match kind {
        ButtonKind::Plain => "",
        ButtonKind::Primary => " btn--primary",
        ButtonKind::Danger => " btn--danger",
        ButtonKind::Ghost => " btn--ghost",
    });
    if small {
        class.push_str(" btn--small");
    }
    view! {
        <button class=class type="button" title=move || title.get() disabled=disabled on:click=move |_| on_click.run(())>
            {children()}
        </button>
    }
}

/// A `[x] label` switch.
#[component]
pub fn Toggle(
    /// What it switches.
    label: &'static str,
    /// Whether it is on.
    value: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <button
            class="toggle"
            type="button"
            aria-pressed=move || bool_attr(value.get())
            on:click=move |_| value.update(|v| *v = !*v)
        >
            {label}
        </button>
    }
}

/// A row of exclusive options.
#[component]
pub fn Segmented<T>(
    /// What the options choose, for screen readers.
    label: &'static str,
    /// Each option and its name.
    options: Vec<(T, &'static str)>,
    /// The option chosen.
    value: RwSignal<T>,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    view! {
        <div class="segmented" role="group" aria-label=label>
            {options
                .into_iter()
                .map(|(option, name)| {
                    view! {
                        <button
                            class="segmented__option"
                            type="button"
                            aria-pressed=move || bool_attr(value.get() == option)
                            on:click=move |_| value.set(option)
                        >
                            {name}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

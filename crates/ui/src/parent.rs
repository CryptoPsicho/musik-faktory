//! Parents' control room: first-time setup, login, spending limits, pricing,
//! API key, account check, spending log and password.

use crate::studio::{Fader, Lcd, Meter, Toggle};
use crate::{api, util, Ctx};
use dioxus::prelude::*;
use musik_shared::*;

#[component]
pub fn Parent() -> Element {
    let ctx = use_context::<Ctx>();
    // Leaving this page locks the control room again (see `Shell`).
    let st = ctx.status.read().clone();
    let unlocked = ctx.token.read().is_some();
    rsx! {
        document::Title { "Parents · Musik Faktory" }
        match (st, unlocked) {
            (None, _) => rsx! { p { class: "muted center", "Connecting to the studio…" } },
            (Some(st), _) if st.setup_needed => rsx! { SetupPanel {} },
            (Some(_), false) => rsx! { LoginPanel {} },
            (Some(_), true) => rsx! { ControlRoom {} },
        }
    }
}

/// Apply only the fields the parent had changed (base → edited) on top of the
/// freshly loaded settings, so changes made meanwhile elsewhere survive.
fn reapply_edits(base: &Settings, edited: &Settings, fresh: &Settings) -> Settings {
    let (b, e, mut f) = (serde_json::to_value(base), serde_json::to_value(edited), serde_json::to_value(fresh));
    if let (Ok(b), Ok(e), Ok(f)) = (&b, &e, &mut f) {
        if let (Some(b), Some(e), Some(fo)) = (b.as_object(), e.as_object(), f.as_object_mut()) {
            for (k, ev) in e {
                if b.get(k) != Some(ev) {
                    fo.insert(k.clone(), ev.clone());
                }
            }
        }
        if let Ok(s) = serde_json::from_value(f.clone()) {
            return s;
        }
    }
    fresh.clone()
}

/// The current parent token (read at call time, so a renewed token is used).
fn token(ctx: Ctx) -> String {
    ctx.token.peek().clone().unwrap_or_default()
}

fn parse_money(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

#[component]
fn SetupPanel() -> Element {
    let ctx = use_context::<Ctx>();
    let mut pw = use_signal(String::new);
    let mut pw2 = use_signal(String::new);
    let mut key = use_signal(String::new);
    let mut settings = use_signal(Settings::default);
    let mut err = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let s = settings();

    let submit = move |_| {
        if pw().chars().count() < 6 {
            err.set(Some("Choose a password with at least 6 characters.".into()));
            return;
        }
        if pw() != pw2() {
            err.set(Some("The two passwords are different.".into()));
            return;
        }
        busy.set(true);
        let req = SetupReq { password: pw(), api_key: key(), settings: Some(settings()) };
        spawn(async move {
            match api::admin_setup(&req).await {
                Ok(r) => {
                    ctx.set_token(Some(r.token));
                    ctx.refresh_status();
                }
                Err(e) => err.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "control-room",
            div { class: "panel span-all",
                div { class: "panel-head", span { class: "title", "First-time setup (parents only)" } }
                div { class: "panel-body stack",
                    p { "Welcome! Set a parent password and the spending rules. Your child can make music, but only you can change these settings." }
                    ol { class: "steps",
                        li { "Create an API key at " a { href: "https://elevenlabs.io/app/settings/api-keys", target: "_blank", "elevenlabs.io → API keys" } "." }
                        li { b { "Recommended:" } " restrict the key to the " b { "Music" } " endpoints and give it a " b { "credit quota" } ". ElevenLabs then enforces a hard cap, even if the key were ever copied." }
                        li { "Optionally also allow “User → read” so this app can measure the exact cost of each song (otherwise it estimates)." }
                    }
                }
            }
            div { class: "panel",
                div { class: "panel-head", span { class: "title", "Parent password" } }
                div { class: "panel-body stack",
                    div { class: "field",
                        label { "Password" }
                        input { r#type: "password", value: "{pw}", oninput: move |e| pw.set(e.value()) }
                    }
                    div { class: "field",
                        label { "Repeat password" }
                        input { r#type: "password", value: "{pw2}", oninput: move |e| pw2.set(e.value()) }
                    }
                    div { class: "field",
                        label { "ElevenLabs API key (can be added later)" }
                        input { r#type: "password", placeholder: "sk_…", value: "{key}", oninput: move |e| key.set(e.value()) }
                    }
                }
            }
            div { class: "panel",
                div { class: "panel-head", span { class: "title", "Spending rules" } }
                div { class: "panel-body stack",
                    LimitField {
                        label: "Weekly limit",
                        value: s.weekly_limit,
                        currency: s.currency.clone(),
                        on_change: move |v| settings.with_mut(|s| s.weekly_limit = v),
                    }
                    LimitField {
                        label: "Monthly limit",
                        value: s.monthly_limit,
                        currency: s.currency.clone(),
                        on_change: move |v| settings.with_mut(|s| s.monthly_limit = v),
                    }
                    div { class: "field",
                        label { "Longest song allowed" }
                        Fader {
                            value: s.max_track_seconds as f64, min: 10.0, max: 300.0, step: 5.0,
                            display: format_duration(s.max_track_seconds * 1000),
                            on_change: move |v: f64| settings.with_mut(|s| s.max_track_seconds = v as u32),
                        }
                    }
                    p { class: "hint", "At ElevenLabs API prices (about {format_money(s.price_per_minute, &s.currency)} per minute) a 60-second song costs about {format_money(estimate_cost(60_000, s.price_per_minute), &s.currency)}. You can fine-tune prices later." }
                }
            }
            div { class: "span-all row end",
                if let Some(e) = err() { span { class: "error", "{e}" } }
                button { class: "btn primary big", disabled: busy(), onclick: submit, "🔒 Save and lock" }
            }
        }
    }
}

#[component]
fn LimitField(label: String, value: Option<f64>, currency: String, on_change: EventHandler<Option<f64>>) -> Element {
    let mut text = use_signal(|| value.map(|v| format!("{v:.2}")).unwrap_or_else(|| "2.00".into()));
    rsx! {
        div { class: "field",
            label { "{label}" }
            div { class: "row",
                Toggle {
                    on: value.is_some(),
                    label: if value.is_some() { "On".to_string() } else { "Off (no limit)".to_string() },
                    on_change: move |on: bool| on_change.call(if on { parse_money(&text()).or(Some(2.0)) } else { None }),
                }
                if value.is_some() {
                    span { class: "muted", "{currency}" }
                    input {
                        r#type: "number", min: "0", step: "0.10", style: "width: 120px",
                        value: "{text}",
                        oninput: move |e| {
                            text.set(e.value());
                            if let Some(v) = parse_money(&e.value()) {
                                on_change.call(Some(v));
                            }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn LoginPanel() -> Element {
    let ctx = use_context::<Ctx>();
    let mut pw = use_signal(String::new);
    let mut err = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut login = move || {
        if busy() {
            return;
        }
        busy.set(true);
        let p = pw();
        spawn(async move {
            match api::admin_login(p).await {
                Ok(r) => {
                    let mut n = ctx.admin_notice;
                    n.set(None);
                    ctx.set_token(Some(r.token));
                }
                Err(e) => err.set(Some(e.message)),
            }
            pw.set(String::new());
            busy.set(false);
        });
    };
    rsx! {
        div { class: "panel login",
            div { class: "panel-head", span { class: "title", "Control room" } }
            div { class: "panel-body stack",
                div { class: "lock", "🔒" }
                p { class: "center muted", "Parents only. Enter the parent password." }
                if let Some(n) = ctx.admin_notice.read().clone() { div { class: "banner warn", "{n}" } }
                input {
                    r#type: "password",
                    placeholder: "Password",
                    value: "{pw}",
                    autofocus: true,
                    oninput: move |e| pw.set(e.value()),
                    onkeydown: move |e| if e.key() == Key::Enter { login() },
                }
                if let Some(e) = err() { p { class: "error", "{e}" } }
                button { class: "btn primary", disabled: busy(), onclick: move |_| login(), "Unlock" }
                p { class: "tiny muted", "Forgot it? Stop the app and run " code { "musik-faktory reset-password" } "." }
            }
        }
    }
}

#[component]
fn ControlRoom() -> Element {
    let ctx = use_context::<Ctx>();
    let mut view = use_signal(|| None::<AdminView>);
    let mut draft = use_signal(|| None::<Settings>);
    let mut ledger = use_signal(Vec::<LedgerEntry>::new);
    let mut msg = use_signal(|| None::<(bool, String)>);
    let mut reload = use_signal(|| 0u32);
    // Bumped whenever the draft is replaced from outside the form fields, so
    // fields with their own text state start over.
    let mut epoch = use_signal(|| 0u32);

    // 401 → back to the login screen, keeping unsaved settings.
    let on_err = move |e: api::ApiErr| {
        if e.unauthorized() {
            let unsaved = match (draft.peek().clone(), view.peek().as_ref()) {
                (Some(d), Some(v)) if d != v.settings => Some((v.settings.clone(), d)),
                _ => None,
            };
            ctx.session_expired(unsaved);
        } else {
            let mut msg = msg;
            msg.set(Some((false, e.message)));
        }
    };

    let _loader = use_resource(move || {
        let _ = reload();
        async move {
            match api::admin_view(&token(ctx)).await {
                Ok(v) => {
                    let dirty = matches!((draft.peek().as_ref(), view.peek().as_ref()), (Some(d), Some(o)) if *d != o.settings);
                    let kept = ctx.kept_draft.peek().clone();
                    if let Some((base, edited)) = kept {
                        let mut kd = ctx.kept_draft;
                        kd.set(None);
                        draft.set(Some(reapply_edits(&base, &edited, &v.settings)));
                        epoch += 1;
                        msg.set(Some((true, "Your unsaved changes are back. Press “Save settings” to keep them.".into())));
                    } else if !dirty {
                        draft.set(Some(v.settings.clone()));
                        epoch += 1;
                    }
                    view.set(Some(v));
                }
                Err(e) => on_err(e),
            }
            if let Ok(l) = api::admin_ledger(&token(ctx)).await {
                ledger.set(l);
            }
        }
    });

    let (Some(v), Some(s)) = (view(), draft()) else {
        return rsx! { p { class: "muted center", "Opening the control room…" } };
    };
    let dirty = s != v.settings;
    // No key yet: the key panel comes first and is highlighted.
    let key_missing = v.api_key_masked.is_none() && !v.mock_mode;

    let save = move |_| {
        let Some(s) = draft() else { return };
        spawn(async move {
            match api::admin_save_settings(&token(ctx), &s).await {
                Ok(v) => {
                    draft.set(Some(v.settings.clone()));
                    view.set(Some(v));
                    msg.set(Some((true, "Saved ✓".into())));
                    ctx.refresh_status();
                }
                Err(e) => on_err(e),
            }
        });
    };
    let logout = move |_| {
        let t = token(ctx);
        spawn(async move {
            let _ = api::admin_logout(&t).await;
            ctx.set_token(None);
        });
    };
    let mut set = move |f: Box<dyn FnOnce(&mut Settings)>| {
        draft.with_mut(|d| {
            if let Some(d) = d.as_mut() {
                f(d)
            }
        })
    };

    rsx! {
        div { class: "row between", style: "margin-bottom: 14px",
            h2 { "🎚 Control room" }
            div { class: "row",
                if v.mock_mode { span { class: "badge", "MOCK MODE" } }
                button { class: "btn ghost small", onclick: logout, "🔓 Lock & leave" }
            }
        }
        if let Some((ok, m)) = msg() {
            div { class: if ok { "banner info" } else { "banner error" }, "{m}" button { class: "btn ghost small", onclick: move |_| msg.set(None), "✕" } }
        }
        div { class: "control-room",
            if key_missing {
                ApiKeyPanel { masked: None, from_env: false, highlight: true, on_changed: move |_| reload += 1, on_err }
            }
            div { class: "panel span-all",
                div { class: "panel-head", span { class: "title", "Spending meters" } }
                div { class: "panel-body two",
                    PeriodMeter { name: "This week", period: v.budget.week.clone(), currency: s.currency.clone() }
                    PeriodMeter { name: "This month", period: v.budget.month.clone(), currency: s.currency.clone() }
                }
            }

            div { class: "panel",
                div { class: "panel-head", span { class: "title", "Limits" } }
                div { class: "panel-body stack",
                    Toggle {
                        on: s.generation_enabled,
                        label: if s.generation_enabled { "Studio open (making songs allowed)".to_string() } else { "Studio closed (paused)".to_string() },
                        on_change: move |b: bool| set(Box::new(move |s| s.generation_enabled = b)),
                    }
                    {
                        rsx! {
                            LimitField {
                                key: "w{epoch}",
                                label: "Weekly limit",
                                value: s.weekly_limit,
                                currency: s.currency.clone(),
                                on_change: move |x: Option<f64>| set(Box::new(move |s| s.weekly_limit = x)),
                            }
                        }
                    }
                    {
                        rsx! {
                            LimitField {
                                key: "m{epoch}",
                                label: "Monthly limit",
                                value: s.monthly_limit,
                                currency: s.currency.clone(),
                                on_change: move |x: Option<f64>| set(Box::new(move |s| s.monthly_limit = x)),
                            }
                        }
                    }
                    div { class: "field",
                        label { "Longest song allowed" }
                        Fader {
                            value: s.max_track_seconds as f64, min: 5.0, max: 600.0, step: 5.0,
                            display: format_duration(s.max_track_seconds * 1000),
                            on_change: move |x: f64| set(Box::new(move |s| s.max_track_seconds = x as u32)),
                        }
                        span { class: "hint", "Always sent explicitly to ElevenLabs, so songs are never longer than this." }
                    }
                    Toggle {
                        on: s.week_starts_sunday,
                        label: if s.week_starts_sunday { "Week starts on Sunday".to_string() } else { "Week starts on Monday".to_string() },
                        on_change: move |b: bool| set(Box::new(move |s| s.week_starts_sunday = b)),
                    }
                }
            }

            div { class: "panel",
                div { class: "panel-head", span { class: "title", "Pricing" } }
                div { class: "panel-body stack",
                    div { class: "two",
                        div { class: "field",
                            label { "Currency symbol" }
                            input { r#type: "text", maxlength: "5", value: "{s.currency}", oninput: move |e| { let c = e.value(); set(Box::new(move |s| s.currency = c)) } }
                        }
                        div { class: "field",
                            label { "Price per minute of music" }
                            input {
                                r#type: "number", min: "0", step: "0.01", value: "{s.price_per_minute}",
                                oninput: move |e| if let Some(x) = parse_money(&e.value()) { set(Box::new(move |s| s.price_per_minute = x)) },
                            }
                        }
                    }
                    div { class: "field",
                        label { "Credits per minute of music" }
                        input {
                            r#type: "number", min: "0", step: "1", value: "{s.credits_per_minute}",
                            oninput: move |e| if let Some(x) = parse_money(&e.value()) { set(Box::new(move |s| s.credits_per_minute = x)) },
                        }
                    }
                    Toggle {
                        on: s.measure_actual_cost,
                        label: "Measure the real cost (credit counter)",
                        title: "Reads your ElevenLabs credit counter before and after each song",
                        on_change: move |b: bool| set(Box::new(move |s| s.measure_actual_cost = b)),
                    }
                    p { class: "hint", "ElevenLabs API list price: " b { "$0.15 per minute" } ", about " b { "900 credits per minute" } " of music. On a subscription, use your plan's price per credit × 900." }
                }
            }

            div { class: "panel",
                div { class: "panel-head", span { class: "title", "Studio defaults" } }
                div { class: "panel-body stack",
                    div { class: "field",
                        label { "Default model" }
                        select {
                            value: "{s.default_model}",
                            onchange: move |e| { let m = e.value(); set(Box::new(move |s| s.default_model = m)) },
                            for (id, name) in MODELS.iter() { option { key: "{id}", value: "{id}", selected: s.default_model == *id, "{name}" } }
                        }
                    }
                    Toggle {
                        on: s.store_for_inpainting,
                        label: "Remember songs for part-editing",
                        title: "Needed for “keep this part / sound similar” in later takes",
                        on_change: move |b: bool| set(Box::new(move |s| s.store_for_inpainting = b)),
                    }
                    p { class: "hint", "Data folder (back this up): " code { "{v.data_dir}" } }
                }
            }

            div { class: "span-all row end",
                if dirty { span { class: "muted", "Unsaved changes" } }
                button {
                    class: "btn ghost",
                    disabled: !dirty,
                    onclick: move |_| {
                        draft.set(Some(v.settings.clone()));
                        epoch += 1;
                    },
                    "Undo"
                }
                button { class: "btn primary big", disabled: !dirty, onclick: save, "💾 Save settings" }
            }

            if !key_missing {
                ApiKeyPanel { masked: v.api_key_masked.clone(), from_env: v.api_key_from_env, highlight: false, on_changed: move |_| reload += 1, on_err }
            }
            AccountPanel { on_err }
            LedgerPanel { entries: ledger(), currency: s.currency.clone(), on_changed: move |l: Vec<LedgerEntry>| { ledger.set(l); reload += 1; ctx.refresh_status(); }, on_err }
            PasswordPanel { on_err }
        }
    }
}

#[component]
fn PeriodMeter(name: String, period: Option<PeriodStatus>, currency: String) -> Element {
    let Some(p) = period else {
        return rsx! { div { class: "meter-block", b { "{name}" } span { class: "muted", "No limit" } } };
    };
    let used = p.spent + p.reserved;
    let frac = if p.limit > 0.0 { used / p.limit } else { 1.0 };
    rsx! {
        div { class: "meter-block",
            div { class: "row", b { "{name}" } span { class: "muted tiny", "refills {util::fmt_day(&p.resets_at)}" } }
            Meter { fraction: frac, segments: 24, wide: true }
            div { class: "row",
                Lcd { label: "SPENT", value: format_money(p.spent, &currency), tone: "amber" }
                if p.reserved > 0.0 { Lcd { label: "RECORDING", value: format_money(p.reserved, &currency) } }
                Lcd { label: "LIMIT", value: format_money(p.limit, &currency) }
                Lcd { label: "LEFT", value: format_money(p.remaining, &currency) }
            }
        }
    }
}

#[component]
fn ApiKeyPanel(masked: Option<String>, from_env: bool, highlight: bool, on_changed: EventHandler<()>, on_err: EventHandler<api::ApiErr>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut key = use_signal(String::new);
    let mut result = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let save = move |_| {
        busy.set(true);
        let k = key();
        spawn(async move {
            match api::admin_set_key(&token(ctx), k).await {
                Ok(r) => {
                    result.set(Some(r.check));
                    key.set(String::new());
                    on_changed.call(());
                    ctx.refresh_status();
                }
                Err(e) if e.unauthorized() => on_err.call(e),
                Err(e) => result.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let clear = move |_| {
        if !util::confirm("Remove the ElevenLabs key? Making songs stops until a new key is added.") {
            return;
        }
        spawn(async move {
            match api::admin_clear_key(&token(ctx)).await {
                Ok(_) => {
                    result.set(Some("Key removed.".into()));
                    on_changed.call(());
                    ctx.refresh_status();
                }
                Err(e) if e.unauthorized() => on_err.call(e),
                Err(e) => result.set(Some(e.message)),
            }
        });
    };
    rsx! {
        div { class: if highlight { "panel span-all attention" } else { "panel" },
            div { class: "panel-head", span { class: "title", if highlight { "🔑 Add your ElevenLabs key" } else { "ElevenLabs key" } } }
            div { class: "panel-body stack",
                if highlight {
                    ol { class: "steps",
                        li { "Open " a { href: "https://elevenlabs.io/app/settings/api-keys", target: "_blank", "elevenlabs.io → Settings → API keys" } " and create a key." }
                        li { "Recommended: allow only the " b { "Music" } " endpoints (plus “User → read” for exact costs) and set a " b { "credit quota" } " as a hard cap." }
                        li { "Paste it below and press “Save & test”." }
                    }
                }
                div { class: "row",
                    Lcd { label: "CURRENT KEY", value: masked.clone().unwrap_or_else(|| "— none —".into()), tone: if masked.is_some() { "" } else { "red" } }
                    if from_env { span { class: "tag", "from ELEVENLABS_API_KEY" } }
                }
                input { r#type: "password", placeholder: "Paste a new key (sk_…)", value: "{key}", oninput: move |e| key.set(e.value()) }
                div { class: "row",
                    button { class: "btn primary small", disabled: busy() || key().trim().is_empty(), onclick: save, "Save & test" }
                    if masked.is_some() && !from_env { button { class: "btn danger small", onclick: clear, "Remove key" } }
                }
                if let Some(r) = result() { p { class: "small", "{r}" } }
                p { class: "hint", "The key stays on the server and is never sent to the browser. For a hard safety net, set a " b { "credit quota" } " on the key in your ElevenLabs dashboard." }
            }
        }
    }
}

#[component]
fn AccountPanel(on_err: EventHandler<api::ApiErr>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut info = use_signal(|| None::<Result<AccountInfo, String>>);
    let check = move |_| {
        spawn(async move {
            match api::admin_account(&token(ctx)).await {
                Err(e) if e.unauthorized() => on_err.call(e),
                r => info.set(Some(r.map_err(|e| e.message))),
            }
        });
    };
    rsx! {
        div { class: "panel",
            div { class: "panel-head", span { class: "title", "ElevenLabs account" } }
            div { class: "panel-body stack",
                button { class: "btn small", onclick: check, "📡 Check account" }
                match info() {
                    None => rsx! { p { class: "hint", "Shows your plan and how many credits are used this billing period." } },
                    Some(Err(e)) => rsx! { p { class: "error small", "{e}" } },
                    Some(Ok(a)) => {
                        let frac = if a.credits_limit > 0 { a.credits_used as f64 / a.credits_limit as f64 } else { 0.0 };
                        rsx! {
                            div { class: "row",
                                Lcd { label: "PLAN", value: a.tier.to_uppercase() }
                                Lcd { label: "CREDITS USED", value: format!("{} / {}", a.credits_used, a.credits_limit), tone: "amber" }
                            }
                            Meter { fraction: frac, segments: 24, wide: true }
                            if let Some(r) = &a.next_reset { p { class: "hint", "Credits reset {util::fmt_day(r)}." } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn LedgerPanel(entries: Vec<LedgerEntry>, currency: String, on_changed: EventHandler<Vec<LedgerEntry>>, on_err: EventHandler<api::ApiErr>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut amount = use_signal(|| "1.00".to_string());
    let mut note = use_signal(String::new);
    let mut err = use_signal(|| None::<String>);
    let mut adjust = move |sign: f64| {
        let Some(a) = parse_money(&amount()).filter(|a| *a > 0.0) else {
            err.set(Some("Enter an amount.".into()));
            return;
        };
        let n = note();
        spawn(async move {
            match api::admin_adjust(&token(ctx), a * sign, n).await {
                Ok(l) => {
                    err.set(None);
                    note.set(String::new());
                    on_changed.call(l);
                }
                Err(e) if e.unauthorized() => on_err.call(e),
                Err(e) => err.set(Some(e.message)),
            }
        });
    };
    // In-flight takes are reserved, not spent yet.
    let total: f64 = entries.iter().filter(|e| !e.pending).map(|e| e.cost).sum();
    rsx! {
        div { class: "panel span-all",
            div { class: "panel-head", span { class: "title", "Spending log" } span { class: "push", "total {format_money(total, &currency)}" } }
            div { class: "panel-body stack",
                div { class: "row",
                    span { class: "muted small", "Adjust the budget:" }
                    span { class: "muted", "{currency}" }
                    input { r#type: "number", min: "0", step: "0.10", style: "width: 110px", value: "{amount}", oninput: move |e| amount.set(e.value()) }
                    input { r#type: "text", placeholder: "Note (e.g. birthday bonus)", style: "flex: 1; min-width: 160px", value: "{note}", oninput: move |e| note.set(e.value()) }
                    button { class: "btn cyan small", title: "Gives budget back (counts as negative spending)", onclick: move |_| adjust(-1.0), "🎁 Give extra budget" }
                    button { class: "btn small", title: "Counts as spending", onclick: move |_| adjust(1.0), "➕ Add a charge" }
                }
                if let Some(e) = err() { p { class: "error small", "{e}" } }
                if entries.is_empty() {
                    p { class: "muted", "Nothing spent yet." }
                } else {
                    div { style: "overflow-x: auto",
                        table { class: "log",
                            thead { tr { th { "When" } th { "Song" } th { "Take" } th { "Length" } th { "Estimate" } th { "Credits" } th { "Charged" } th { "Note" } } }
                            tbody {
                                for e in entries.iter() {
                                    tr { key: "{e.id}", class: if e.kind == LedgerKind::Adjustment { "adj" } else if e.pending { "pending" } else { "" },
                                        td { "{util::fmt_datetime(&e.at)}" }
                                        td { "{e.track_title}" }
                                        td { "{e.version_label}" }
                                        td { if e.seconds > 0.0 { "{format_duration((e.seconds * 1000.0) as u32)}" } }
                                        td { if e.kind == LedgerKind::Generation { "{format_money(e.estimated_cost, &currency)}" } }
                                        td { if let Some(c) = e.measured_credits { "{c}" } else { "–" } }
                                        td { if e.pending { "recording…" } else { "{format_money(e.cost, &currency)}" } }
                                        td { "{e.note}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PasswordPanel(on_err: EventHandler<api::ApiErr>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut cur = use_signal(String::new);
    let mut new = use_signal(String::new);
    let mut msg = use_signal(|| None::<String>);
    let change = move |_| {
        let (c, n) = (cur(), new());
        spawn(async move {
            match api::admin_password(&token(ctx), c, n).await {
                Ok(r) => {
                    cur.set(String::new());
                    new.set(String::new());
                    msg.set(Some("Password changed ✓".into()));
                    ctx.set_token(Some(r.token));
                }
                Err(e) if e.unauthorized() => on_err.call(e),
                Err(e) => msg.set(Some(e.message)),
            }
        });
    };
    rsx! {
        div { class: "panel",
            div { class: "panel-head", span { class: "title", "Parent password" } }
            div { class: "panel-body stack",
                input { r#type: "password", placeholder: "Current password", value: "{cur}", oninput: move |e| cur.set(e.value()) }
                input { r#type: "password", placeholder: "New password (6+ characters)", value: "{new}", oninput: move |e| new.set(e.value()) }
                button { class: "btn small", onclick: change, "Change password" }
                if let Some(m) = msg() { p { class: "small", "{m}" } }
            }
        }
    }
}

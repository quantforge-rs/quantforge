//! The alert templates: the text an operator pastes as an alert's message,
//! rendered the way TradingView renders it and read back as the signal the
//! shared cases record.

use quantforge::protocol::signal::{Signal, SignalKind, SignalProblemCode, SignalSide};

use crate::cases::{Template, fixture};

/// The placeholders a template may use: the ones TradingView documents for
/// any alert, each rendered the same way every time the alert is sent.
/// None is a strategy's: no template asks what an order was.
fn documented(placeholder: &str) -> bool {
    let numbered = || {
        placeholder
            .strip_prefix("{{plot_")
            .and_then(|rest| rest.strip_suffix("}}"))
            .and_then(|number| number.parse::<u32>().ok())
            .is_some_and(|number| number <= 19)
    };
    let named = || {
        placeholder
            .strip_prefix("{{plot(\"")
            .and_then(|rest| rest.strip_suffix("\")}}"))
            .is_some_and(|name| !name.is_empty() && !name.contains('"'))
    };
    [
        "{{ticker}}",
        "{{exchange}}",
        "{{time}}",
        "{{timenow}}",
        "{{close}}",
    ]
    .contains(&placeholder)
        || numbered()
        || named()
}

/// The template's text with each placeholder replaced, as TradingView
/// sends it.
fn rendered(template: &Template, with: impl Fn(&str) -> String) -> String {
    template
        .placeholders
        .iter()
        .fold(template.template.clone(), |text, placeholder| {
            text.replace(placeholder, &with(placeholder))
        })
}

#[test]
fn the_templates_are_an_entry_and_an_exit_for_each_side() {
    let fixture = fixture();
    let names: Vec<&str> = fixture
        .templates
        .iter()
        .map(|template| template.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "entry-long",
            "entry-short",
            "entry-long-with-stop",
            "entry-short-with-stop",
            "exit-long",
            "exit-short",
        ]
    );
}

#[test]
fn every_template_renders_to_the_signal_the_cases_record() {
    let fixture = fixture();
    for template in &fixture.templates {
        let name = &template.name;
        let text = rendered(template, |placeholder| {
            fixture.rendered_with[placeholder].clone()
        });
        assert_eq!(text, template.rendered, "{name}");
        let signal =
            Signal::read(text.as_bytes()).unwrap_or_else(|problems| panic!("{name}: {problems}"));
        assert_eq!(signal.signal_id(), template.signal_id, "{name}");
        assert_eq!(signal.canonical(), template.canonical, "{name}");

        // What the template is named for is what it sends, as constants.
        let kind = if name.starts_with("entry") {
            SignalKind::Entry
        } else {
            SignalKind::Exit
        };
        let side = if name.contains("long") {
            SignalSide::Long
        } else {
            SignalSide::Short
        };
        assert_eq!((signal.kind(), signal.side()), (kind, side), "{name}");
        for constant in [
            format!("\"kind\": \"{kind}\""),
            format!("\"side\": \"{side}\""),
            "\"version\": 1".to_string(),
            "\"profile\": \"main\"".to_string(),
            "\"source\": \"trend\"".to_string(),
        ] {
            assert!(template.template.contains(&constant), "{name}: {constant}");
        }
        assert_eq!(
            signal.stop().is_some(),
            name.ends_with("with-stop"),
            "{name}"
        );
        assert_eq!(
            signal.price().is_some(),
            kind == SignalKind::Entry,
            "{name}"
        );
    }
}

#[test]
fn a_template_uses_only_what_tradingview_renders_the_same_on_every_send() {
    let fixture = fixture();
    for template in &fixture.templates {
        let name = &template.name;
        let text = &template.template;
        // Every placeholder in the text is one the cases list, and each is
        // documented for any alert.
        let found: Vec<&str> = text
            .match_indices("{{")
            .map(|(at, _)| {
                let end = text[at..].find("}}").expect("a closed placeholder") + at + 2;
                &text[at..end]
            })
            .collect();
        assert_eq!(found, template.placeholders, "{name}");
        for placeholder in &found {
            assert!(documented(placeholder), "{name}: {placeholder}");
            // In quotes, so that whatever is rendered is a JSON string.
            assert!(
                text.contains(&format!("\"{placeholder}\"")),
                "{name}: {placeholder}"
            );
        }
        // The ID is built from the bar's time; the fire time only dates it.
        assert!(text.contains("\"bar_time\": \"{{time}}\""), "{name}");
        assert!(text.contains("\"fire_time\": \"{{timenow}}\""), "{name}");
        assert!(text.contains("\"ticker\": \"{{ticker}}\""), "{name}");
        // Direction is never read from an order.
        assert!(!text.contains("strategy."), "{name}");
    }
    assert!(documented("{{plot_19}}") && !documented("{{plot_20}}"));
    assert!(!documented("{{strategy.order.action}}"));
    assert!(!documented("{{plot(\"\")}}"));
}

#[test]
fn a_template_that_was_not_rendered_is_not_a_signal() {
    let fixture = fixture();
    for template in &fixture.templates {
        let refused = Signal::read(template.template.as_bytes()).expect_err("not a signal");
        let codes: Vec<SignalProblemCode> = refused
            .problems()
            .iter()
            .map(|problem| problem.code())
            .collect();
        // A plot named in quotes is not even JSON until it is rendered.
        let expected = if template.template.contains("{{plot(\"") {
            vec![SignalProblemCode::NotJson]
        } else {
            vec![SignalProblemCode::InvalidValue; template.placeholders.len()]
        };
        assert_eq!(codes, expected, "{}", template.name);
    }
}

#[test]
fn a_placeholder_rendered_as_no_value_refuses_the_alert() {
    // What an indicator renders where a plot has no value is not
    // documented. Whatever it is, the alert is refused: nothing is read
    // as "no stop", which would put the profile's fallback in its place.
    let fixture = fixture();
    let with_stop = fixture
        .templates
        .iter()
        .find(|template| template.name == "entry-long-with-stop")
        .expect("the template");
    for missing in ["NaN", "na", "n/a", "null", "", "0", "-1", " "] {
        let text = rendered(with_stop, |placeholder| {
            if placeholder == "{{plot_0}}" {
                missing.to_string()
            } else {
                fixture.rendered_with[placeholder].clone()
            }
        });
        let refused = Signal::read(text.as_bytes()).expect_err("refused");
        let found: Vec<_> = refused
            .problems()
            .iter()
            .map(|problem| (problem.code(), problem.field_name()))
            .collect();
        assert_eq!(
            found,
            [(SignalProblemCode::InvalidValue, Some("stop"))],
            "{missing:?}"
        );
    }
    // A price TradingView renders with a different number of places is
    // the price it renders: nothing is rounded or trimmed.
    for close in ["234.2", "234.20", "234.230000"] {
        let text = rendered(with_stop, |placeholder| {
            if placeholder == "{{close}}" {
                close.to_string()
            } else {
                fixture.rendered_with[placeholder].clone()
            }
        });
        let signal = Signal::read(text.as_bytes()).expect("a signal");
        assert_eq!(
            signal.price().map(|price| price.to_string()).as_deref(),
            Some(close)
        );
    }
}

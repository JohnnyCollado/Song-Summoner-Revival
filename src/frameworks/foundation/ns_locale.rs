/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSLocale`.

use super::{ns_array, ns_string};
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_foundation::cf_locale::{kCFLocaleCountryCode, kCFLocaleIdentifier};
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};
use crate::window::{get_preferred_country_codes, get_preferred_language_codes};
use crate::Environment;

const NSLocaleCountryCode: &str = "NSLocaleCountryCode";
const NSLocaleIdentifier: &str = "NSLocaleIdentifier";
const NSLocaleLanguageCode: &str = "NSLocaleLanguageCode";

pub const CONSTANTS: ConstantExports = &[
    (
        "_NSLocaleCountryCode",
        HostConstant::NSString(NSLocaleCountryCode),
    ),
    (
        "_NSLocaleIdentifier",
        HostConstant::NSString(NSLocaleIdentifier),
    ),
    (
        "_NSLocaleLanguageCode",
        HostConstant::NSString(NSLocaleLanguageCode),
    ),
];

/// Map a few common ISO language codes to their English display names. Used by
/// [-NSLocale displayNameForKey:value:]. Anything unknown falls back to the
/// input code so the caller still gets a non-nil string.
fn language_display_name(code: &str) -> &'static str {
    match code.to_ascii_lowercase().as_str() {
        "en" => "English",
        "ja" => "Japanese",
        "fr" => "French",
        "de" => "German",
        "it" => "Italian",
        "es" => "Spanish",
        "zh" => "Chinese",
        "ko" => "Korean",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "nl" => "Dutch",
        "sv" => "Swedish",
        "da" => "Danish",
        "no" => "Norwegian",
        "fi" => "Finnish",
        "pl" => "Polish",
        _ => "",
    }
}

#[derive(Default)]
pub struct State {
    current_locale: Option<id>,
    system_locale: Option<id>,
    preferred_languages: Option<id>,
}
impl State {
    fn get(env: &mut Environment) -> &mut State {
        &mut env.framework_state.foundation.ns_locale
    }
}

/// Use `msg_class![env; NSLocale preferredLanguages]` rather than calling this
/// directly, because it may be slow and there is no caching.
fn get_preferred_languages(env: &mut Environment) -> Vec<String> {
    let options = env.options.as_ref();
    if let Some(ref preferred_languages) = options.preferred_languages {
        log!("The app requested your preferred languages. {:?} will reported based on your --preferred-languages= option.", preferred_languages);
        return preferred_languages.clone();
    }

    let languages = get_preferred_language_codes(env);
    if languages.is_empty() {
        let lang = "en".to_string();
        log!("The app requested your preferred languages. No information could be retrieved, so {:?} (English) will be reported.", lang);
        vec![lang]
    } else {
        log!("The app requested your preferred languages. {:?} will be reported based on your system language preferences.", languages);
        languages
    }
}

fn get_preferred_countries(env: &mut Environment) -> Vec<String> {
    let countries = get_preferred_country_codes(env);
    if countries.is_empty() {
        let country = "US".to_string();
        log!("The app requested your current locale. No country information could be retrieved, so {:?} will be reported.", country);
        vec![country]
    } else {
        log!("The app requested your current locale. {:?} will be reported based on your system region settings.", countries);
        countries
    }
}

struct NSLocaleHostObject {
    /// `NSString *`
    country_code: id,
    /// `NSString *`
    language_code: id,
}
impl HostObject for NSLocaleHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSLocale: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSLocaleHostObject {
        country_code: nil,
        language_code: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// The documentation isn't clear about what the format of the strings should be,
// but Super Monkey Ball does `isEqualToString:` against "fr", "es", "de", "it"
// and "ja", and its locale detection works properly, so presumably they do not
// usually have region suffixes.
+ (id)preferredLanguages {
    if let Some(existing) = State::get(env).preferred_languages {
        existing
    } else {
        let langs = get_preferred_languages(env);
        let lang_ns_strings = langs.into_iter().map(|lang| ns_string::from_rust_string(env, lang)).collect();
        let new = ns_array::from_vec(env, lang_ns_strings);
        State::get(env).preferred_languages = Some(new);
        new
    }
}

+ (id)currentLocale {
    if let Some(locale) = State::get(env).current_locale {
        locale
    } else {
        let countries = get_preferred_countries(env);
        let country_code = ns_string::from_rust_string(env, countries[0].clone());
        let languages = get_preferred_languages(env);
        let language_code = ns_string::from_rust_string(env, languages[0].clone());
        let host_object = NSLocaleHostObject {
            country_code,
            language_code,
        };
        let new_locale = env.objc.alloc_object(
            this,
            Box::new(host_object),
            &mut env.mem
        );
        State::get(env).current_locale = Some(new_locale);
        new_locale
    }
}

+ (id)systemLocale {
    if let Some(locale) = State::get(env).system_locale {
        locale
    } else {
        let host_object = NSLocaleHostObject {
            // Was confirmed on the iOS Simulator
            country_code: nil,
            language_code: nil,
        };
        let new_locale = env.objc.alloc_object(
            this,
            Box::new(host_object),
            &mut env.mem
        );
        State::get(env).system_locale = Some(new_locale);
        new_locale
    }
}

// TODO: constructors, more accessors

- (id)initWithLocaleIdentifier:(id)string { // NSString *
    let identifier = ns_string::to_rust_string(env, string).to_string();
    log_dbg!("[(NSLocale *){:?} initWithLocaleIdentifier:'{}']", this, identifier);

    // Accept "lang", "lang_REGION", "lang-region", or longer subtag forms;
    // we only extract the language and (optional) region subtags.
    let normalized = identifier.replace('-', "_");
    let mut parts = normalized.split('_');
    let lang = parts.next().unwrap_or("").to_ascii_lowercase();
    let region = parts.next().unwrap_or("").to_ascii_uppercase();
    assert!(!lang.is_empty(), "empty language subtag in locale identifier");

    let language_code = ns_string::from_rust_string(env, lang);
    let country_code = if region.is_empty() {
        nil
    } else {
        ns_string::from_rust_string(env, region)
    };

    assert!(env.objc.borrow::<NSLocaleHostObject>(this).language_code == nil);
    let host = env.objc.borrow_mut::<NSLocaleHostObject>(this);
    host.language_code = language_code;
    host.country_code = country_code;
    this
}

- (())dealloc {
    let &NSLocaleHostObject { country_code, language_code } = env.objc.borrow::<NSLocaleHostObject>(this);
    release(env, country_code);
    release(env, language_code);
    env.objc.dealloc_object(this, &mut env.mem)
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

- (id)localeIdentifier {
    let locale_id_key = ns_string::get_static_str(env, NSLocaleIdentifier);
    msg![env; this objectForKey:locale_id_key]
}

- (id)objectForKey:(id)key {
    let key_str: &str = &ns_string::to_rust_string(env, key);
    match key_str {
        // Note: this is not the cleanest separation between NS and CF parts
        // But it does work on the iOS Simulator
        // TODO: Define NSLocaleCountryCode _as_ kCFLocaleCountryCode
        NSLocaleCountryCode | kCFLocaleCountryCode => {
            let &NSLocaleHostObject { country_code, .. } = env.objc.borrow(this);
            country_code
        },
        NSLocaleLanguageCode => {
            let &NSLocaleHostObject { language_code, .. } = env.objc.borrow(this);
            language_code
        },
        // TODO: Define NSLocaleIdentifier _as_ kCFLocaleIdentifier
        NSLocaleIdentifier | kCFLocaleIdentifier => {
            let &NSLocaleHostObject { country_code, language_code } = env.objc.borrow(this);
            assert!(language_code != nil); // TODO
            let lang = ns_string::to_rust_string(env, language_code).to_string();
            let locale_id_str = if country_code == nil {
                lang
            } else {
                format!(
                    "{}_{}",
                    lang,
                    ns_string::to_rust_string(env, country_code)
                )
            };
            let res = ns_string::from_rust_string(env, locale_id_str);
            autorelease(env, res)
        },
        _ => unimplemented!("[NSLocale objectForKey:'{}'] is not implemented", key_str)
    }
}

// `displayNameForKey:value:` returns a human-readable name for the given
// language/country code. We only handle language codes; anything else (or an
// unknown code) returns the input value unchanged, which is good enough for
// apps that just compare against expected names like "English".
- (id)displayNameForKey:(id)key value:(id)value {
    let key_str = ns_string::to_rust_string(env, key).to_string();
    let value_str = ns_string::to_rust_string(env, value).to_string();
    log_dbg!(
        "[(NSLocale*){:?} displayNameForKey:'{}' value:'{}']",
        this, key_str, value_str
    );
    if key_str == NSLocaleLanguageCode {
        let mapped = language_display_name(&value_str);
        if !mapped.is_empty() {
            let res = ns_string::from_rust_string(env, mapped.to_string());
            return autorelease(env, res);
        }
    }
    // Fallback: hand back the input code as-is.
    retain(env, value);
    autorelease(env, value)
}

@end

};

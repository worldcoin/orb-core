//! MECARD format for WiFi credentials.
//!
//! Spec:
//! <https://github.com/zxing/zxing/wiki/Barcode-Contents#wi-fi-network-config-android-ios-11>

use nom::{
    branch::alt,
    bytes::complete::tag,
    character::complete::anychar,
    combinator::{eof, fail, map, opt, verify},
    multi::fold_many1,
    sequence::pair,
    IResult,
};
use std::{
    fmt::{self, Debug},
    ops::Deref,
    str,
};

/// WiFi network credentials.
#[derive(Debug)]
pub struct Credentials {
    /// Authentication type.
    pub auth_type: AuthType,
    /// Network SSID.
    pub ssid: String,
    /// Password.
    pub password: Option<Password>,
    /// Whether the network SSID is hidden.
    pub hidden: bool,
}

/// Authentication type.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum AuthType {
    /// WEP encryption.
    Wep,
    /// WPA encryption.
    Wpa,
    /// Pure WPA3-SAE.
    Sae,
    /// Unencrypted.
    Nopass,
}

impl Default for AuthType {
    fn default() -> Self {
        Self::Nopass
    }
}

/// Newtype on `String` to prevent printing in plaintext.
#[derive(Clone, Hash, Eq, PartialEq)]
pub struct Password(pub String);

impl Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl Deref for Password {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq<&str> for Password {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl Credentials {
    /// Parses WiFi credentials encoded in MECARD format.
    pub fn parse(input: &str) -> IResult<&str, Self> {
        let (mut input, _) = tag("WIFI:")(input)?;

        // Parses a set of fields with the following requirements:
        // 1. A field is parsed no more than once.
        // 2. Fields are parsed in arbitrary order.
        // 3. Each field is optional.
        macro_rules! parse_fields {
            ($($parse:path => $opt:ident,)*) => {
                $(let mut $opt = None;)*
                loop {
                    $(
                        if $opt.is_none() {
                            if let Ok((next_input, parsed)) = $parse(input) {
                                $opt = Some(parsed);
                                input = next_input;
                                continue;
                            }
                        }
                    )*
                    break;
                }
            };
        }
        parse_fields! {
            AuthType::parse => auth_type,
            parse_ssid => ssid,
            parse_password => password,
            parse_hidden => hidden,
        }
        let password = password.map(Password);
        // ssid is actually not optional.
        if ssid.is_none() {
            let (_, ()) = fail(input)?;
        }

        let (input, _) = tag(";")(input)?;
        let (input, _) = eof(input)?;

        let auth_type = auth_type.unwrap_or_default();
        let ssid = ssid.unwrap_or_default();
        let hidden = hidden.unwrap_or_default();
        Ok((input, Self { auth_type, ssid, password, hidden }))
    }
}

impl AuthType {
    fn parse(input: &str) -> IResult<&str, Self> {
        parse_field(input, "T", |input| {
            let wep = map(tag("WEP"), |_| Self::Wep);
            let wpa = map(tag("WPA"), |_| Self::Wpa);
            let sae = map(tag("SAE"), |_| Self::Sae);
            let nopass = map(tag("nopass"), |_| Self::Nopass);
            alt((wep, wpa, sae, nopass))(input)
        })
    }
}

fn parse_ssid(input: &str) -> IResult<&str, String> {
    parse_field(input, "S", parse_string)
}

fn parse_password(input: &str) -> IResult<&str, String> {
    parse_field(input, "P", parse_string)
}

fn parse_hidden(input: &str) -> IResult<&str, bool> {
    parse_field(input, "H", |input| {
        let true_val = map(tag("true"), |_| true);
        let false_val = map(tag("false"), |_| false);
        alt((true_val, false_val))(input)
    })
}

fn parse_string(input: &str) -> IResult<&str, String> {
    const SPECIAL_CHARS: &[char] = &['\\', ';', ',', '"', ':'];
    let non_special = verify(anychar, |c| SPECIAL_CHARS.iter().all(|s| c != s));
    let special = pair(tag("\\"), verify(anychar, |c| SPECIAL_CHARS.iter().any(|s| c == s)));
    let unescaped = alt((non_special, map(special, |(_, c)| c)));
    let (input, quote) = opt(tag("\""))(input)?;
    let (input, string) = fold_many1(unescaped, String::new, |mut acc, item| {
        acc.push(item);
        acc
    })(input)?;
    if quote.is_some() {
        let (input, _) = tag("\"")(input)?;
        Ok((input, string))
    } else if string.len() % 2 == 0 && string.chars().all(|c| c.is_ascii_hexdigit()) {
        // The value is in hex string format.
        let string = string.as_bytes().chunks(2).fold(
            String::with_capacity(string.len() / 2),
            |mut acc, pair| {
                // The following sequence of unwraps can't fail because of the
                // condition above.
                let string = str::from_utf8(pair).unwrap();
                let octet = u8::from_str_radix(string, 16).unwrap();
                let chr = char::from_u32(octet.into()).unwrap();
                acc.push(chr);
                acc
            },
        );
        Ok((input, string))
    } else {
        Ok((input, string))
    }
}

fn parse_field<'input, 'name, T, F: FnOnce(&'input str) -> IResult<&'input str, T>>(
    input: &'input str,
    name: &'name str,
    f: F,
) -> IResult<&'input str, T> {
    let (input, _) = tag(name)(input)?;
    let (input, _) = tag(":")(input)?;
    let (input, value) = f(input)?;
    let (input, _) = tag(";")(input)?;
    Ok((input, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple() {
        let input = "WIFI:T:WPA;S:mynetwork;P:mypass;;";
        let (_, credentials) = Credentials::parse(input).unwrap();
        assert_eq!(credentials.auth_type, AuthType::Wpa);
        assert_eq!(credentials.ssid, "mynetwork");
        assert_eq!(credentials.password.unwrap(), "mypass");
        assert!(!credentials.hidden);
    }

    #[test]
    fn test_escaped() {
        let input = r#"WIFI:S:\"foo\;bar\\baz\";;"#;
        let (_, credentials) = Credentials::parse(input).unwrap();
        assert_eq!(credentials.auth_type, AuthType::Nopass);
        assert_eq!(credentials.ssid, r#""foo;bar\baz""#);
        assert_eq!(credentials.password, None);
        assert!(!credentials.hidden);
    }

    #[test]
    fn test_quoted() {
        let input = r#"WIFI:S:"\"foo\;bar\\baz\"";P:"mypass";;"#;
        let (_, credentials) = Credentials::parse(input).unwrap();
        assert_eq!(credentials.auth_type, AuthType::Nopass);
        assert_eq!(credentials.ssid, r#""foo;bar\baz""#);
        assert_eq!(credentials.password.unwrap(), "mypass");
        assert!(!credentials.hidden);
    }

    #[test]
    fn test_unescaped() {
        let input = r#"WIFI:S:"foo;bar\baz";;"#;
        assert!(Credentials::parse(input).is_err());
    }

    #[test]
    fn test_different_order() {
        let input = "WIFI:P:mypass;H:true;S:mynetwork;T:WPA;;";
        let (_, credentials) = Credentials::parse(input).unwrap();
        assert_eq!(credentials.auth_type, AuthType::Wpa);
        assert_eq!(credentials.ssid, "mynetwork");
        assert_eq!(credentials.password.unwrap(), "mypass");
        assert!(credentials.hidden);
    }

    #[test]
    fn test_missing_ssid() {
        let input = "WIFI:P:mypass;T:WPA;H:true;;";
        assert!(Credentials::parse(input).is_err());
    }

    #[test]
    fn test_duplicates() {
        let input = "WIFI:H:true;P:mypass;T:WPA;S:mynetwork;P:mypass;;";
        assert!(Credentials::parse(input).is_err());
    }

    #[test]
    fn test_trailing_garbage() {
        let input = "WIFI:T:WPA;S:mynetwork;P:mypass;;garbage";
        assert!(Credentials::parse(input).is_err());
    }

    #[test]
    fn test_hex_string() {
        let input = r"WIFI:S:776f726c64636f696e;;";
        let (_, credentials) = Credentials::parse(input).unwrap();
        assert_eq!(credentials.auth_type, AuthType::Nopass);
        assert_eq!(credentials.ssid, "worldcoin");
        assert_eq!(credentials.password, None);
        assert!(!credentials.hidden);
    }
}

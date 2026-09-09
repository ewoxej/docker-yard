pub mod service;
pub mod env;
pub mod config;

pub use service::parse_service_file;
pub use env::parse_env_file;
pub use config::parse_config_file;

use nom::{
    branch::alt,
    bytes::complete::{take_until, take_while, take_while1},
    character::complete::{char, multispace0, multispace1},
    combinator::recognize,
    sequence::{delimited, preceded, tuple},
    IResult,
};

/// Common parser combinators
pub fn ws(input: &str) -> IResult<&str, &str> {
    multispace0(input)
}

pub fn ws1(input: &str) -> IResult<&str, &str> {
    multispace1(input)
}

pub fn lexeme<'a, F, O>(mut parser: F) -> impl FnMut(&'a str) -> IResult<&'a str, O>
where
    F: FnMut(&'a str) -> IResult<&'a str, O>,
{
    move |input| {
        let (input, output) = parser(input)?;
        let (input, _) = ws(input)?;
        Ok((input, output))
    }
}

pub fn quoted_string(input: &str) -> IResult<&str, String> {
    alt((
        delimited(char('"'), take_until("\""), char('"')),
        delimited(char('\''), take_until("'"), char('\'')),
    ))(input)
    .map(|(i, s)| (i, s.to_string()))
}

pub fn identifier(input: &str) -> IResult<&str, String> {
    recognize(tuple((
        take_while1(|c: char| c.is_alphabetic() || c == '_'),
        take_while(|c: char| c.is_alphanumeric() || c == '_' || c == '-'),
    )))(input)
    .map(|(i, s)| (i, s.to_string()))
}

pub fn skip_comment(input: &str) -> IResult<&str, &str> {
    preceded(char('#'), take_until("\n"))(input)
}

pub fn line_content(input: &str) -> IResult<&str, &str> {
    let (input, content) = take_until("\n")(input)?;
    // Strip comments
    if let Some(pos) = content.find("# ") {
        Ok((input, &content[..pos]))
    } else {
        Ok((input, content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identifier() {
        assert_eq!(
            identifier("nextcloud "),
            Ok((" ", "nextcloud".to_string()))
        );
        assert_eq!(
            identifier("service-name "),
            Ok((" ", "service-name".to_string()))
        );
    }

    #[test]
    fn test_quoted_string() {
        assert_eq!(
            quoted_string("\"hello world\""),
            Ok(("", "hello world".to_string()))
        );
        assert_eq!(
            quoted_string("'single quotes'"),
            Ok(("", "single quotes".to_string()))
        );
    }
}

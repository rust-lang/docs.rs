/*!
This is the [Font Awesome Free](https://fontawesome.com/how-to-use/on-the-web/setup/hosting-font-awesome-yourself) SVG files as a crate.

This is not officially supported by Fonticons, Inc.
If you have problems, [contact us](https://github.com/rust-lang/docs.rs/issues), not them.
*/

use std::error::Error;
use std::fmt::{self, Debug, Display, Formatter};

#[cfg(font_awesome_out_dir)]
include!(concat!(env!("OUT_DIR"), "/fontawesome.rs"));
#[cfg(not(font_awesome_out_dir))]
include!("fontawesome.rs");

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum Type {
    Brands,
    Regular,
    Solid,
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct TypeError;

impl Display for TypeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid Font Awesome icon type: must be one of brands, regular, or solid"
        )
    }
}

impl Error for TypeError {}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct NameError;

impl Display for NameError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid Font Awesome icon name: visit https://fontawesome.com/icons?d=gallery&m=free to see valid names"
        )
    }
}

impl Error for NameError {}

impl Type {
    pub const fn as_str(self) -> &'static str {
        match self {
            Type::Brands => "brands",
            Type::Regular => "regular",
            Type::Solid => "solid",
        }
    }
}

impl std::str::FromStr for Type {
    type Err = TypeError;
    fn from_str(s: &str) -> Result<Type, TypeError> {
        match s {
            "brands" => Ok(Type::Brands),
            "regular" => Ok(Type::Regular),
            "solid" => Ok(Type::Solid),
            _ => Err(TypeError),
        }
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_str(), f)
    }
}

/**
Get a fontawesome svg file by its name.
*/
pub const fn svg(type_: Type, name: &str) -> Result<&'static str, NameError> {
    let svg = fontawesome_svg(type_.as_str(), name);
    if svg.is_empty() {
        return Err(NameError);
    }
    Ok(svg)
}

pub trait IconStr {
    /// Name of the icon, like "triangle-exclamation".
    fn icon_name(&self) -> &'static str;
    /// The SVG content of the icon.
    fn icon_svg(&self) -> &'static str;
}

pub trait Brands: IconStr + Debug {
    fn get_type() -> Type {
        Type::Brands
    }
}
pub trait Regular: IconStr + Debug {
    fn get_type() -> Type {
        Type::Regular
    }
}
pub trait Solid: IconStr + Debug {
    fn get_type() -> Type {
        Type::Solid
    }
}

#[cfg(test)]
mod tests {
    const fn usable_as_const_() {
        assert!(crate::svg(crate::Type::Solid, "gear").is_ok());
    }
    #[test]
    fn usable_as_const() {
        usable_as_const_();
    }
    #[test]
    fn it_works() {
        assert!(crate::svg(crate::Type::Solid, "gear").is_ok());
        assert!(crate::svg(crate::Type::Solid, "download").is_ok());
        assert!(crate::svg(crate::Type::Solid, "gibberish").is_err());
    }
}

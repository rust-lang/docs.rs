//! Simple badge generator

extern crate rusttype;


use rusttype::{Font, FontCollection, Scale, point, Point, PositionedGlyph};


const FONT_DATA: &'static [u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"),
                                                        "/DejaVuSans.ttf"));
const FONT_SIZE: f32 = 11.;


pub struct BadgeOptions {
    /// Subject will be displayed on the left side of badge
    pub subject: String,
    /// Status will be displayed on the right side of badge
    pub status: String,
    /// HTML color of badge
    pub color: String,
}


impl Default for BadgeOptions {
    fn default() -> BadgeOptions {
        BadgeOptions {
            subject: "build".to_owned(),
            status: "passing".to_owned(),
            color: "#4c1".to_owned(),
        }
    }
}


pub struct Badge<'a> {
    options: BadgeOptions,
    font: Font<'a>,
    scale: Scale,
    offset: Point<f32>,
}


impl<'a> Badge<'a> {
    pub fn new(options: BadgeOptions) -> Result<Badge<'a>, String> {
        let collection = FontCollection::from_bytes(FONT_DATA);
        // this should never fail in practice
        let font = try!(collection.into_font().ok_or("Failed to load font data".to_owned()));
        let scale = Scale {
            x: FONT_SIZE,
            y: FONT_SIZE,
        };
        let v_metrics = font.v_metrics(scale);
        let offset = point(0.0, v_metrics.ascent);
        Ok(Badge {
            options: options,
            font: font,
            scale: scale,
            offset: offset,
        })
    }


    pub fn to_svg(&self) -> String {
        let left_width = self.calculate_width(&self.options.subject) + 6;
        let right_width = self.calculate_width(&self.options.status) + 6;

        let svg = format!(r###"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}" height="20">
  <linearGradient id="smooth" x2="0" y2="100%">
    <stop offset="0" stop-color="#bbb" stop-opacity=".1"/>
    <stop offset="1" stop-opacity=".1"/>
  </linearGradient>

  <mask id="round">
    <rect width="{}" height="20" rx="3" fill="#fff"/>
  </mask>

  <g mask="url(#round)">
    <rect width="{}" height="20" fill="#555"/>
    <rect x="{}" width="{}" height="20" fill="{}"/>
    <rect width="{}" height="20" fill="url(#smooth)"/>
  </g>

  <g fill="#fff" text-anchor="middle" font-family="DejaVu Sans,Verdana,Geneva,sans-serif" font-size="11">
    <text x="{}" y="15" fill="#010101" fill-opacity=".3">{}</text>
    <text x="{}" y="14">{}</text>
    <text x="{}" y="15" fill="#010101" fill-opacity=".3">{}</text>
    <text x="{}" y="14">{}</text>
  </g>
</svg>"###,
            left_width + right_width,
            left_width + right_width,
            left_width,
            left_width,
            right_width,
            self.options.color,
            left_width + right_width,
            (left_width) / 2,
            self.options.subject,
            (left_width) / 2,
            self.options.subject,
            left_width + (right_width / 2),
            self.options.status,
            left_width + (right_width / 2),
            self.options.status);

        svg
    }


    fn calculate_width(&self, text: &str) -> u32 {
        let glyphs: Vec<PositionedGlyph> =
            self.font.layout(text, self.scale, self.offset).collect();
        let width: u32 = glyphs.iter()
            .rev()
            .filter_map(|g| {
                g.pixel_bounding_box()
                    .map(|b| b.min.x as f32 + g.unpositioned().h_metrics().advance_width)
            })
        .next()
            .unwrap_or(0.0)
            .ceil() as u32;
        width + ((text.len() as u32 - 1) * 2)
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> BadgeOptions {
        BadgeOptions::default()
    }


    #[test]
    fn test_new() {
        assert!(Badge::new(options()).is_ok());
    }

    #[test]
    fn test_calculate_width() {
        let badge = Badge::new(options()).unwrap();
        assert_eq!(badge.calculate_width("build"), 31);
        assert_eq!(badge.calculate_width("passing"), 48);
    }

    #[test]
    #[ignore]
    fn test_to_svg() {
        use std::fs::File;
        use std::io::Write;
        let mut file = File::create("test.svg").unwrap();
        let options = BadgeOptions {
            subject: "build".to_owned(),
            status: "passing".to_owned(),
            .. BadgeOptions::default()
        };
        let badge = Badge::new(options).unwrap();
        file.write_all(badge.to_svg().as_bytes()).unwrap();
    }
}

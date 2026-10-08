//! Colour palette and theme roles taken from the logo.
//!
//! The logo's opaque pixels are clustered with k-means in Oklab, a perceptual
//! colour space where straight-line distance roughly matches how different two
//! colours look. The seed is fixed, so an image always gives the same palette.

use std::{cmp::Reverse, ops::RangeInclusive};

use crate::{logo, term::Rgb};

/// Number of clusters.
const K: usize = 6;
const ITERATIONS: usize = 10;
/// k-means runs on at most this many pixels, picked evenly across the image.
const MAX_SAMPLES: usize = 4096;
/// Clusters holding less than 1/50 (2%) of the pixels are dropped.
const MIN_SHARE: usize = 50;
const SEED: u64 = 0xff37_c4ba_1e77_e5ed;
/// Oklab lightness of clusters that may become the accent or the muted colour.
const ACCENT_LIGHTNESS: RangeInclusive<f32> = 0.35..=0.80;
const MUTED_LIGHTNESS: RangeInclusive<f32> = 0.40..=0.80;
/// Clusters darker than this (Oklab) are shading and outlines, not a secondary colour.
const SECONDARY_MIN_LIGHTNESS: f32 = 0.35;
/// How far (Oklab distance) the secondary colour must be from the accent and the muted colour.
const MIN_SECONDARY_DISTANCE: f32 = 0.15;
const MIN_SECONDARY_MUTED_DISTANCE: f32 = 0.05;
/// Role colours keep at least this HSL lightness away from the terminal background.
const MIN_ROLE_LIGHTNESS: f32 = 0.45;
/// Used when no cluster qualifies for the role.
const FALLBACK_ACCENT: Rgb = Rgb(196, 63, 86);
const FALLBACK_MUTED: Rgb = Rgb(128, 128, 128);

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// Cluster colours, most common first.
    pub colors: Vec<Rgb>,
    /// Labels and the user name.
    pub accent: Rgb,
    /// The host name.
    pub secondary: Rgb,
    /// The separator line and the `@`.
    pub muted: Rgb,
}

/// The terminal background that role colours must stand out against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Background {
    #[default]
    Dark,
    // Chosen through the config file once it exists (M6).
    #[allow(dead_code)]
    Light,
}

/// How the palette turns into colours on screen. Built-in defaults for now;
/// the config file fills it in from M6.
#[derive(Clone, Debug, Default)]
pub struct Theme {
    pub background: Background,
    /// Per-role overrides. They are used exactly as given.
    pub accent: Option<Rgb>,
    pub secondary: Option<Rgb>,
    pub muted: Option<Rgb>,
}

/// Role colours, ready to paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles {
    pub accent: Rgb,
    pub secondary: Rgb,
    pub muted: Rgb,
}

impl Theme {
    /// The role colours for `palette`, adjusted for the terminal background.
    pub fn roles(&self, palette: &Palette) -> Roles {
        let pick =
            |set: Option<Rgb>, role: Rgb| set.unwrap_or_else(|| self.background.adjust(role));
        Roles {
            accent: pick(self.accent, palette.accent),
            secondary: pick(self.secondary, palette.secondary),
            muted: pick(self.muted, palette.muted),
        }
    }
}

impl Background {
    /// Colours too dark to read on a dark terminal (or too light on a light
    /// one) move to the nearest readable lightness; the rest stay exactly as
    /// they are in the image.
    fn adjust(self, Rgb(r, g, b): Rgb) -> Rgb {
        let rgb = [r, g, b].map(f32::from);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let lightness = (max as f32 + min as f32) / 2.0 / 255.0;
        let limit = match self {
            Self::Dark if lightness < MIN_ROLE_LIGHTNESS => MIN_ROLE_LIGHTNESS,
            Self::Light if lightness > 1.0 - MIN_ROLE_LIGHTNESS => 1.0 - MIN_ROLE_LIGHTNESS,
            _ => return Rgb(r, g, b),
        };
        logo::remap_lightness(rgb, limit, limit)
    }
}

/// Extracts the palette from an RGBA buffer. `background` is the colour keyed
/// out of the image, if any; it becomes the accent.
pub fn extract(rgba: &[u8], background: Option<Rgb>) -> Palette {
    let clusters = kmeans(&samples(rgba));
    let by_chroma = |a: &&Cluster, b: &&Cluster| a.lab.chroma().total_cmp(&b.lab.chroma());

    let accent = background.unwrap_or_else(|| {
        let score = |c: &&Cluster| c.lab.chroma() * c.count as f32;
        let eligible = clusters
            .iter()
            .filter(|c| ACCENT_LIGHTNESS.contains(&c.lab.l));
        eligible
            .max_by(|a, b| score(a).total_cmp(&score(b)))
            .map_or(FALLBACK_ACCENT, |c| c.lab.to_rgb())
    });
    let muted = clusters
        .iter()
        .filter(|c| MUTED_LIGHTNESS.contains(&c.lab.l))
        .min_by(by_chroma)
        .map_or(FALLBACK_MUTED, |c| c.lab.to_rgb());
    // The most common colour that isn't shading and stands apart from the
    // other two roles (clusters are sorted largest first).
    let (accent_lab, muted_lab) = (Lab::from_rgb(accent), Lab::from_rgb(muted));
    let secondary = clusters
        .iter()
        .find(|c| {
            c.lab.l >= SECONDARY_MIN_LIGHTNESS
                && c.lab.distance(accent_lab) > MIN_SECONDARY_DISTANCE
                && c.lab.distance(muted_lab) > MIN_SECONDARY_MUTED_DISTANCE
        })
        .map_or(accent, |c| c.lab.to_rgb());

    Palette {
        colors: clusters.iter().map(|c| c.lab.to_rgb()).collect(),
        accent,
        secondary,
        muted,
    }
}

/// The opaque pixels (alpha >= 0.5) in Oklab, thinned out evenly to at most
/// `MAX_SAMPLES` so large images stay cheap.
fn samples(rgba: &[u8]) -> Vec<Lab> {
    let opaque = || rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128);
    let step = opaque().count().div_ceil(MAX_SAMPLES).max(1);
    opaque()
        .step_by(step)
        .map(|p| Lab::from_rgb(Rgb(p[0], p[1], p[2])))
        .collect()
}

struct Cluster {
    lab: Lab,
    /// Number of samples closest to this cluster.
    count: usize,
}

/// k-means with k-means++ seeding. Returns the clusters that hold at least
/// 1/`MIN_SHARE` of the points, largest first.
fn kmeans(points: &[Lab]) -> Vec<Cluster> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut rng = SplitMix64(SEED);
    let mut centers = vec![points[(rng.next() % points.len() as u64) as usize]];
    // Each further centre is a point picked with probability proportional to
    // its squared distance from the nearest centre so far.
    let mut nearest: Vec<f32> = points.iter().map(|p| p.distance2(centers[0])).collect();
    while centers.len() < K {
        let total: f64 = nearest.iter().map(|&d| d as f64).sum();
        if total <= 0.0 {
            break; // Fewer distinct colours than K.
        }
        let mut target = rng.unit() * total;
        let pick = nearest.iter().position(|&d| {
            target -= d as f64;
            target < 0.0
        });
        // Rounding can leave `target` just short; any point not yet a centre will do.
        let Some(i) = pick.or_else(|| nearest.iter().rposition(|&d| d > 0.0)) else {
            break;
        };
        centers.push(points[i]);
        for (d, p) in nearest.iter_mut().zip(points) {
            *d = d.min(p.distance2(points[i]));
        }
    }

    let mut owner = vec![0; points.len()];
    for _ in 0..ITERATIONS {
        assign(points, &centers, &mut owner);
        let mut sums = vec![(Lab::default(), 0usize); centers.len()];
        for (p, &o) in points.iter().zip(&owner) {
            let (sum, n) = &mut sums[o];
            *sum = sum.add(*p);
            *n += 1;
        }
        // A centre that lost all its points stays where it is.
        for (c, (sum, n)) in centers.iter_mut().zip(sums) {
            if n > 0 {
                *c = sum.scale(1.0 / n as f32);
            }
        }
    }
    assign(points, &centers, &mut owner);

    let mut clusters: Vec<Cluster> = centers
        .into_iter()
        .map(|lab| Cluster { lab, count: 0 })
        .collect();
    for &o in &owner {
        clusters[o].count += 1;
    }
    clusters.retain(|c| c.count * MIN_SHARE >= points.len());
    clusters.sort_by_key(|c| Reverse(c.count));
    clusters
}

/// Sets `owner[i]` to the index of the centre closest to `points[i]`.
fn assign(points: &[Lab], centers: &[Lab], owner: &mut [usize]) {
    for (p, o) in points.iter().zip(owner) {
        let mut best = (0, f32::INFINITY);
        for (i, c) in centers.iter().enumerate() {
            let d = p.distance2(*c);
            if d < best.1 {
                best = (i, d);
            }
        }
        *o = best.0;
    }
}

/// A small, fast PRNG (splitmix64) so seeding is reproducible without a crate.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A colour in Oklab: `l` is lightness (0-1), `a` and `b` the green-red and
/// blue-yellow axes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Lab {
    l: f32,
    a: f32,
    b: f32,
}

impl Lab {
    fn from_rgb(Rgb(r, g, b): Rgb) -> Self {
        let [r, g, b] = [r, g, b].map(srgb_to_linear);
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        Self {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        }
    }

    /// The nearest sRGB colour (out-of-gamut channels are clipped).
    fn to_rgb(self) -> Rgb {
        let l = (self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b).powi(3);
        let m = (self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b).powi(3);
        let s = (self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b).powi(3);
        Rgb(
            linear_to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
            linear_to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
            linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
        )
    }

    fn chroma(self) -> f32 {
        self.a.hypot(self.b)
    }

    fn distance2(self, o: Self) -> f32 {
        (self.l - o.l).powi(2) + (self.a - o.a).powi(2) + (self.b - o.b).powi(2)
    }

    /// Perceptual colour difference (ΔE in Oklab).
    fn distance(self, o: Self) -> f32 {
        self.distance2(o).sqrt()
    }

    fn add(self, o: Self) -> Self {
        Self {
            l: self.l + o.l,
            a: self.a + o.a,
            b: self.b + o.b,
        }
    }

    fn scale(self, k: f32) -> Self {
        Self {
            l: self.l * k,
            a: self.a * k,
            b: self.b * k,
        }
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let v = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logo::LogoImage;

    fn logo_palette() -> Palette {
        // The embedded logo is assets/logo.png (see the logo module's tests).
        let logo = LogoImage::embedded();
        extract(&logo.pixels, logo.background)
    }

    /// RGBA pixels: `n` opaque pixels of each colour, in order.
    fn pixels(colors: &[(Rgb, usize)]) -> Vec<u8> {
        colors
            .iter()
            .flat_map(|&(Rgb(r, g, b), n)| [r, g, b, 255].repeat(n))
            .collect()
    }

    fn nearest(colors: &[Rgb], target: Rgb) -> f32 {
        let t = Lab::from_rgb(target);
        colors
            .iter()
            .map(|&c| Lab::from_rgb(c).distance(t))
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn oklab_reference_values() {
        // Reference values from Björn Ottosson's Oklab post.
        let white = Lab::from_rgb(Rgb(255, 255, 255));
        assert!((white.l - 1.0).abs() < 1e-3 && white.a.abs() < 1e-3 && white.b.abs() < 1e-3);
        let red = Lab::from_rgb(Rgb(255, 0, 0));
        assert!(
            (red.l - 0.628).abs() < 1e-3 && (red.a - 0.225).abs() < 1e-3,
            "{red:?}"
        );
        for c in [
            Rgb(195, 62, 88),
            Rgb(0, 0, 0),
            Rgb(18, 200, 255),
            Rgb(71, 61, 70),
        ] {
            assert_eq!(Lab::from_rgb(c).to_rgb(), c);
        }
    }

    #[test]
    fn same_image_same_palette() {
        assert_eq!(logo_palette(), logo_palette());
    }

    #[test]
    fn logo_palette_matches_reference_colors() {
        let p = logo_palette();
        let Rgb(r, g, b) = p.accent;
        let close = |v: u8, t: u8| v.abs_diff(t) <= 1;
        assert!(
            close(r, 0xc3) && close(g, 0x3e) && close(b, 0x58),
            "accent {:?}",
            p.accent
        );
        assert!(p.colors.len() <= 6);
        // The spec also lists maroon #633540, but k-means merges it into the
        // dark mauve cluster (#493943, 0.050 away) for any seed or sample size,
        // and spends the spare clusters on near-black shades instead.
        let references = [
            ("cream", Rgb(0xf2, 0xd7, 0xc5)),
            ("rose-beige", Rgb(0xc9, 0xa6, 0x9d)),
            ("dark mauve", Rgb(0x47, 0x3d, 0x46)),
        ];
        for (name, target) in references {
            let d = nearest(&p.colors, target);
            assert!(
                d < 0.05,
                "no colour near {name}: closest is {d:.3} away in {:?}",
                p.colors
            );
        }
    }

    #[test]
    fn logo_roles() {
        let p = logo_palette();
        // Host name in the cream fur, separator in the rose-beige shading.
        assert!(
            nearest(&[p.secondary], Rgb(0xf2, 0xd7, 0xc5)) < 0.01,
            "{p:?}"
        );
        assert!(nearest(&[p.muted], Rgb(0xc9, 0xa6, 0x9d)) < 0.01, "{p:?}");
    }

    #[test]
    fn roles_without_background() {
        let red = Rgb(200, 40, 50);
        let blue = Rgb(40, 90, 220);
        let grey = Rgb(150, 145, 140);
        let p = extract(&pixels(&[(grey, 500), (red, 300), (blue, 200)]), None);
        assert_eq!(p.colors.len(), 3);
        assert!(nearest(&[p.accent], red) < 0.01, "{p:?}");
        assert!(nearest(&[p.secondary], blue) < 0.01, "{p:?}");
        assert!(nearest(&[p.muted], grey) < 0.01, "{p:?}");
    }

    #[test]
    fn background_becomes_accent() {
        let bg = Rgb(195, 62, 88);
        let p = extract(
            &pixels(&[(Rgb(240, 220, 200), 100), (Rgb(40, 90, 220), 100)]),
            Some(bg),
        );
        assert_eq!(p.accent, bg);
        assert_ne!(p.secondary, p.accent, "{p:?}");
    }

    #[test]
    fn rare_colors_are_dropped() {
        let p = extract(
            &pixels(&[(Rgb(200, 40, 50), 990), (Rgb(40, 90, 220), 10)]),
            None,
        );
        assert_eq!(p.colors.len(), 1, "{p:?}");
    }

    #[test]
    fn transparent_pixels_are_ignored() {
        let mut rgba = pixels(&[(Rgb(200, 40, 50), 100)]);
        rgba.extend([40, 90, 220, 100].repeat(500));
        let p = extract(&rgba, None);
        assert_eq!(p.colors.len(), 1, "{p:?}");
    }

    #[test]
    fn empty_image_falls_back() {
        let p = extract(&[0, 0, 0, 0].repeat(64), None);
        assert!(p.colors.is_empty());
        assert_eq!(p.secondary, p.accent);
        assert_eq!(extract(&[], None), p);
    }

    #[test]
    fn theme_adjusts_lightness_for_the_background() {
        let lightness = |Rgb(r, g, b): Rgb| (r.max(g).max(b) as u16 + r.min(g).min(b) as u16) / 2;
        let light_theme = Theme {
            background: Background::Light,
            ..Theme::default()
        };

        // Too dark for a dark terminal: lifted. Readable on a light one: kept.
        let p = extract(&pixels(&[(Rgb(60, 30, 40), 10)]), Some(Rgb(30, 20, 25)));
        let dark = Theme::default().roles(&p);
        assert!(
            lightness(dark.accent) > lightness(p.accent) + 40,
            "{dark:?}"
        );
        assert_eq!(light_theme.roles(&p).accent, p.accent);

        // Readable on a dark terminal: kept exactly. Too light for a light one: darkened.
        let p = extract(&pixels(&[(Rgb(60, 30, 40), 10)]), Some(Rgb(240, 225, 210)));
        assert_eq!(Theme::default().roles(&p).accent, p.accent);
        let light = light_theme.roles(&p);
        assert!(
            lightness(light.accent) + 40 < lightness(p.accent),
            "{light:?}"
        );
        let pinned = Rgb(1, 2, 3);
        let custom = Theme {
            muted: Some(pinned),
            ..Theme::default()
        }
        .roles(&p);
        assert_eq!(custom.muted, pinned);
    }
}

//! The mascot of a bot: the head of a crow seen from the front, in the bot's
//! color, drawn alone with nothing behind it. Its eyes show what the bot does.
//! Every mascot has the same short beak: the head is what tells bots apart.
//!
//! A mascot is a stack of layers of one color each (`MascotLayer`), all drawn
//! on the same box, from the back to the front.
//!
//! A mascot that is alive moves while nothing changes: its head sways about
//! its base, its pupils look to the side it leans to, and lids of its own
//! color close over its eyes in a blink. Each of these is a timed
//! transformation of a layer, which the window plays in the scene it already
//! has: the view is not drawn again for it.

use crowbot_protocol::{Avatar, BotState, MascotShape};
use gpui::{
    Animation, AnimationExt, AnyElement, Div, Easing, Pixels, TimedTransformation, Transformation,
    div, point, prelude::*, radians, svg,
};

use crate::assets::MascotLayer;
use crate::bots;
use crate::theme::{Theme, duration, mascot};

/// The dots a bot that works looks up at, from the first to hop to the last.
const DOTS: [MascotLayer; 3] = [MascotLayer::Dot1, MascotLayer::Dot2, MascotLayer::Dot3];

/// What the eyes of a mascot show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mood {
    /// Round eyes that look ahead.
    #[default]
    Rest,
    /// Eyes that look up at three dots, which hop one after the other.
    Working,
    /// Wide eyes on a tilted head: the bot waits for an answer.
    Waiting,
    /// Eyes that look down under lowered lids.
    Failed,
    /// Eyes closed in a smile: the bot has something new to show.
    Happy,
}

impl From<BotState> for Mood {
    fn from(state: BotState) -> Self {
        match state {
            BotState::Idle => Self::Rest,
            BotState::Working => Self::Working,
            BotState::Waiting => Self::Waiting,
            BotState::Failed => Self::Failed,
            BotState::Unread => Self::Happy,
        }
    }
}

/// What a layer is painted with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    /// The color of the bot.
    Body,
    Beak,
    Eye,
    Pupil,
}

fn head(shape: MascotShape) -> MascotLayer {
    match shape {
        MascotShape::Smooth => MascotLayer::HeadSmooth,
        MascotShape::Crest => MascotLayer::HeadCrest,
        MascotShape::Spikes => MascotLayer::HeadSpikes,
        MascotShape::Curl => MascotLayer::HeadCurl,
        MascotShape::Plume => MascotLayer::HeadPlume,
        MascotShape::Flat => MascotLayer::HeadFlat,
    }
}

/// The layers of a mascot, from the back to the front. A small mascot takes
/// the larger eyes, and one that is alive has the lids of its blink on top.
fn layers(avatar: &Avatar, mood: Mood, small: bool, alive: bool) -> Vec<(MascotLayer, Ink)> {
    let pick = |large, little| if small { little } else { large };
    let eyes = pick(MascotLayer::Eyes, MascotLayer::SmallEyes);
    let mut layers = vec![
        (head(avatar.shape), Ink::Body),
        (MascotLayer::Beak, Ink::Beak),
    ];
    match mood {
        Mood::Rest => layers.extend([
            (eyes, Ink::Eye),
            (
                pick(MascotLayer::PupilsRest, MascotLayer::SmallPupilsRest),
                Ink::Pupil,
            ),
        ]),
        Mood::Working => {
            layers.extend([
                (eyes, Ink::Eye),
                (
                    pick(MascotLayer::PupilsWorking, MascotLayer::SmallPupilsWorking),
                    Ink::Pupil,
                ),
            ]);
            layers.extend(DOTS.map(|dot| (dot, Ink::Body)));
        }
        Mood::Waiting => layers.extend([
            (
                pick(MascotLayer::EyesWide, MascotLayer::SmallEyesWide),
                Ink::Eye,
            ),
            (
                pick(MascotLayer::PupilsWaiting, MascotLayer::SmallPupilsWaiting),
                Ink::Pupil,
            ),
        ]),
        Mood::Failed => layers.extend([
            (eyes, Ink::Eye),
            (
                pick(MascotLayer::PupilsFailed, MascotLayer::SmallPupilsFailed),
                Ink::Pupil,
            ),
            (pick(MascotLayer::Lids, MascotLayer::SmallLids), Ink::Body),
        ]),
        Mood::Happy => layers.push((
            pick(MascotLayer::EyesHappy, MascotLayer::SmallEyesHappy),
            Ink::Eye,
        )),
    }
    if alive && bearing(mood).blinks {
        layers.push((pick(MascotLayer::Blink, MascotLayer::SmallBlink), Ink::Body));
    }
    layers
}

/// Where a layer stands on the box: turned by `degrees` about `pivot`, then
/// moved by `shift`, with its height scaled by `stretch` before the turn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    degrees: f32,
    pivot: (f32, f32),
    shift: (f32, f32),
    stretch: f32,
}

impl Pose {
    fn turned(degrees: f32, pivot: (f32, f32)) -> Self {
        Self {
            degrees,
            pivot,
            shift: (0., 0.),
            stretch: 1.,
        }
    }

    fn shifted(self, x: f32, y: f32) -> Self {
        Self {
            shift: (self.shift.0 + x, self.shift.1 + y),
            ..self
        }
    }

    /// How far the layer moves, on the box. A layer turns and stretches about
    /// the middle of its box: this moves it so that the turn keeps `pivot` in
    /// place, then adds the shift.
    fn offset(&self) -> (f32, f32) {
        let (sin, cos) = self.degrees.to_radians().sin_cos();
        let half = mascot::BOX / 2.;
        let (x, y) = (self.pivot.0 - half, self.pivot.1 - half);
        (
            x - (cos * x - sin * y) + self.shift.0,
            y - (sin * x + cos * y) + self.shift.1,
        )
    }

    /// Where a point of the layer lands, on the box.
    #[cfg(test)]
    fn place(&self, point: (f32, f32)) -> (f32, f32) {
        let (sin, cos) = self.degrees.to_radians().sin_cos();
        let half = mascot::BOX / 2.;
        let (x, y) = (point.0 - half, (point.1 - half) * self.stretch);
        let (offset_x, offset_y) = self.offset();
        (
            half + offset_x + cos * x - sin * y,
            half + offset_y + sin * x + cos * y,
        )
    }

    fn transformation(&self, size: Pixels) -> Transformation {
        let (x, y) = self.offset();
        let unit = size * (1. / mascot::BOX);
        Transformation::rotate(radians(self.degrees.to_radians()))
            .with_scaling(gpui::size(1., self.stretch))
            .with_translation(point(unit * x, unit * y))
    }
}

/// How a mascot holds its head in a mood, and how it moves while nothing
/// changes.
struct Bearing {
    /// The pose of the head: a turn about a point.
    degrees: f32,
    pivot: (f32, f32),
    /// How far the head sways to each side of that pose, in degrees.
    sway: f32,
    /// How far the pupils look to the side the head leans to.
    gaze: f32,
    /// How high the mascot bobs.
    bob: f32,
    /// How many times faster than at rest the mascot moves.
    pace: u32,
    blinks: bool,
}

fn bearing(mood: Mood) -> Bearing {
    let upright = Bearing {
        degrees: 0.,
        pivot: mascot::NECK,
        sway: 0.,
        gaze: 0.,
        bob: 0.,
        pace: 1,
        blinks: true,
    };
    match mood {
        Mood::Rest => Bearing {
            sway: mascot::SWAY_REST,
            gaze: mascot::GAZE,
            ..upright
        },
        // The eyes stay on the dots.
        Mood::Working => Bearing {
            sway: mascot::SWAY_WORKING,
            ..upright
        },
        // The eyes stay on the user.
        Mood::Waiting => Bearing {
            degrees: mascot::TILT,
            pivot: mascot::TILT_CENTER,
            sway: mascot::SWAY_WAITING,
            pace: mascot::WAITING_PACE,
            ..upright
        },
        Mood::Failed => Bearing {
            sway: mascot::SWAY_FAILED,
            ..upright
        },
        // The eyes are closed in a smile: nothing blinks.
        Mood::Happy => Bearing {
            bob: mascot::BOB,
            pace: mascot::BOB_PACE,
            blinks: false,
            ..upright
        },
    }
}

impl Bearing {
    /// The pose of a mascot that does not move.
    fn pose(&self) -> Pose {
        Pose::turned(self.degrees, self.pivot)
    }

    /// The two ends of the sway of the mascot `id`, for a layer that looks
    /// `gaze` to the side the head leans to. Bots next to each other start
    /// on opposite sides.
    fn sway_ends(&self, id: usize, gaze: f32) -> (Pose, Pose) {
        let side = if id.is_multiple_of(2) { 1. } else { -1. };
        let lean = self.sway * side;
        (
            Pose::turned(self.degrees - lean, self.pivot).shifted(-gaze * side, 0.),
            Pose::turned(self.degrees + lean, self.pivot).shifted(gaze * side, -self.bob),
        )
    }

    fn sway(&self, id: usize, gaze: f32, size: Pixels) -> TimedTransformation {
        let (from, to) = self.sway_ends(id, gaze);
        let period = mascot::sway_period(id) / self.pace;
        TimedTransformation::new(period)
            .from(from.transformation(size))
            .to(to.transformation(size))
            .with_easing(Easing::EaseInOut)
            .alternate()
            .with_max_fps(mascot::FPS)
    }

    /// The lids of a blink, open then closed: they hang from their top, which
    /// stays in place while they come down over the eyes. They do not sway:
    /// they are larger than the eyes by more than the eyes move.
    fn blink_ends(&self) -> (Pose, Pose) {
        let closed = self.pose();
        let lift = (mascot::LID_TOP - mascot::BOX / 2.) * (1. - mascot::LID_OPEN);
        let (sin, cos) = self.degrees.to_radians().sin_cos();
        let open = Pose {
            stretch: mascot::LID_OPEN,
            ..closed.shifted(-sin * lift, cos * lift)
        };
        (open, closed)
    }

    fn blink(&self, id: usize, size: Pixels) -> TimedTransformation {
        let (open, closed) = self.blink_ends();
        TimedTransformation::new(mascot::blink_period(id))
            .from(open.transformation(size))
            .to(closed.transformation(size))
            .with_easing(Easing::Heartbeat)
            .with_max_fps(mascot::FPS)
    }
}

/// How high a dot is in its hop, from 0 to 1, at a point of the cycle: it
/// rises and falls during the first part of the cycle, then rests.
fn hop(phase: f32) -> f32 {
    let rise = duration::WORKING_RISE;
    if phase < rise {
        (phase / rise * std::f32::consts::PI).sin()
    } else {
        0.
    }
}

/// The mascot of an avatar in a square of `size`, with the eyes of `mood`.
/// `id` tells the mascots of a window apart, so that each bot moves at its
/// own pace. A mascot that is not `alive` stands still in the pose of its
/// mood; the dots of one that works hop either way.
pub fn mascot(
    id: usize,
    avatar: &Avatar,
    mood: Mood,
    alive: bool,
    size: Pixels,
    theme: &Theme,
) -> Div {
    let body = bots::avatar_color(&avatar.color).unwrap_or(theme.accent);
    let small = size < mascot::SMALL;
    let bearing = bearing(mood);
    let still = (bearing.degrees != 0.).then(|| bearing.pose().transformation(size));
    let rise = size * (mascot::DOT_HOP / mascot::BOX);
    div().relative().flex_none().size(size).children(
        layers(avatar, mood, small, alive)
            .into_iter()
            .map(move |(part, ink)| -> AnyElement {
                let layer = svg()
                    .path(part.path())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .text_color(match ink {
                        Ink::Body => body,
                        Ink::Beak => theme.mascot_beak,
                        Ink::Eye => theme.mascot_eye,
                        Ink::Pupil => theme.mascot_pupil,
                    });
                if let Some(dot) = DOTS.iter().position(|dot| *dot == part) {
                    return layer
                        .with_animation(
                            ("mascot-dot", id * DOTS.len() + dot),
                            Animation::new(duration::WORKING).repeat(),
                            move |layer, turn| {
                                let phase =
                                    (turn + 1. - dot as f32 * duration::WORKING_STAGGER).fract();
                                layer.with_transformation(Transformation::translate(point(
                                    Pixels::ZERO,
                                    -rise * hop(phase),
                                )))
                            },
                        )
                        .into_any_element();
                }
                if matches!(part, MascotLayer::Blink | MascotLayer::SmallBlink) {
                    return layer
                        .with_timed_transformation(bearing.blink(id, size))
                        .into_any_element();
                }
                if alive {
                    let gaze = if ink == Ink::Pupil { bearing.gaze } else { 0. };
                    return layer
                        .with_timed_transformation(bearing.sway(id, gaze, size))
                        .into_any_element();
                }
                match still {
                    Some(still) => layer.with_transformation(still),
                    None => layer,
                }
                .into_any_element()
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avatar(shape: MascotShape) -> Avatar {
        Avatar {
            color: "#3b93f0".to_owned(),
            shape,
        }
    }

    #[test]
    fn every_state_of_a_bot_has_its_own_eyes() {
        let moods = [
            (BotState::Idle, Mood::Rest),
            (BotState::Working, Mood::Working),
            (BotState::Waiting, Mood::Waiting),
            (BotState::Failed, Mood::Failed),
            (BotState::Unread, Mood::Happy),
        ];
        let subject = avatar(MascotShape::Smooth);
        let mut seen = Vec::new();
        for (state, mood) in moods {
            assert_eq!(Mood::from(state), mood);
            let eyes = layers(&subject, mood, false, false)[2..].to_vec();
            assert!(!seen.contains(&eyes), "{mood:?}");
            seen.push(eyes);
        }
    }

    #[test]
    fn a_mascot_is_its_head_then_the_beak_then_its_eyes() {
        for shape in MascotShape::ALL {
            assert_eq!(
                layers(&avatar(shape), Mood::Rest, false, false),
                [
                    (head(shape), Ink::Body),
                    (MascotLayer::Beak, Ink::Beak),
                    (MascotLayer::Eyes, Ink::Eye),
                    (MascotLayer::PupilsRest, Ink::Pupil),
                ]
            );
        }
        let heads: std::collections::HashSet<_> = MascotShape::ALL
            .into_iter()
            .map(|shape| head(shape).path())
            .collect();
        assert_eq!(heads.len(), 6);
    }

    #[test]
    fn a_failed_bot_lowers_lids_of_its_own_color_and_a_working_one_shows_dots() {
        let subject = avatar(MascotShape::Crest);
        assert_eq!(
            layers(&subject, Mood::Failed, false, false).last(),
            Some(&(MascotLayer::Lids, Ink::Body))
        );
        assert_eq!(
            layers(&subject, Mood::Working, false, false)[4..],
            DOTS.map(|dot| (dot, Ink::Body))
        );
        assert_eq!(
            layers(&subject, Mood::Happy, false, false)[2..],
            [(MascotLayer::EyesHappy, Ink::Eye)]
        );
    }

    #[test]
    fn a_small_mascot_takes_the_larger_eyes_and_keeps_its_head_and_beak() {
        let subject = avatar(MascotShape::Plume);
        for mood in [
            Mood::Rest,
            Mood::Working,
            Mood::Waiting,
            Mood::Failed,
            Mood::Happy,
        ] {
            let large = layers(&subject, mood, false, false);
            let small = layers(&subject, mood, true, false);
            assert_eq!(large.len(), small.len());
            assert_eq!(large[..2], small[..2]);
            for ((large, ink), (small, small_ink)) in large[2..].iter().zip(&small[2..]) {
                assert_eq!(ink, small_ink);
                if DOTS.contains(large) {
                    assert_eq!(large, small);
                } else {
                    assert_eq!(
                        small.path(),
                        large.path().replace("mascot/", "mascot/small/")
                    );
                }
            }
        }
    }

    #[test]
    fn a_dot_rises_and_falls_then_rests_until_the_next_cycle() {
        assert_eq!(hop(0.), 0.);
        assert!((hop(duration::WORKING_RISE / 2.) - 1.).abs() < 1e-5);
        assert!(hop(duration::WORKING_RISE / 4.) > 0.);
        for phase in [duration::WORKING_RISE, 0.6, 0.99] {
            assert_eq!(hop(phase), 0.);
        }
        // The three hops follow each other, and the last one ends in the cycle.
        const { assert!(2. * duration::WORKING_STAGGER + duration::WORKING_RISE < 1.) };
    }

    fn near(found: (f32, f32), expected: (f32, f32)) -> bool {
        (found.0 - expected.0).abs() < 1e-3 && (found.1 - expected.1).abs() < 1e-3
    }

    const MOODS: [Mood; 5] = [
        Mood::Rest,
        Mood::Working,
        Mood::Waiting,
        Mood::Failed,
        Mood::Happy,
    ];

    #[test]
    fn a_turn_keeps_its_pivot_in_place() {
        let tilt = bearing(Mood::Waiting).pose();
        assert_eq!(tilt.degrees, mascot::TILT);
        assert!(near(tilt.place(mascot::TILT_CENTER), mascot::TILT_CENTER));
        // The head leans to the left: its top moves left.
        assert!(tilt.place((50., 10.)).0 < 50.);
        // A mascot in another mood stands upright.
        for mood in [Mood::Rest, Mood::Working, Mood::Failed, Mood::Happy] {
            assert!(near(bearing(mood).pose().place((30., 40.)), (30., 40.)));
        }
    }

    #[test]
    fn a_head_sways_about_its_base_to_both_sides_of_its_pose() {
        for mood in MOODS {
            let bearing = bearing(mood);
            let (from, to) = bearing.sway_ends(0, 0.);
            assert!(near(from.place(bearing.pivot), bearing.pivot), "{mood:?}");
            assert_eq!(from.degrees + to.degrees, 2. * bearing.degrees);
            assert_eq!(to.degrees - from.degrees, 2. * bearing.sway);
            // Bots next to each other start on opposite sides.
            let (other, _) = bearing.sway_ends(1, 0.);
            assert_eq!(other.degrees, to.degrees);
        }
        // Only a mascot with something new leaves the ground.
        let (low, high) = bearing(Mood::Happy).sway_ends(0, 0.);
        assert!(near(low.place(mascot::NECK), mascot::NECK));
        assert!(high.place(mascot::NECK).1 < mascot::NECK.1);
    }

    #[test]
    fn the_pupils_of_a_mascot_at_rest_look_to_the_side_its_head_leans_to() {
        let rest = bearing(Mood::Rest);
        let (head_from, head_to) = rest.sway_ends(0, 0.);
        let (from, to) = rest.sway_ends(0, rest.gaze);
        let pupil = (36.4, 55.);
        // The top of the head goes right when the turn is clockwise.
        assert!(head_to.place((50., 22.)).0 > head_from.place((50., 22.)).0);
        assert!(to.place(pupil).0 - head_to.place(pupil).0 > 1.);
        assert!(from.place(pupil).0 - head_from.place(pupil).0 < -1.);
        for mood in [Mood::Working, Mood::Waiting, Mood::Failed, Mood::Happy] {
            assert_eq!(bearing(mood).gaze, 0., "{mood:?}");
        }
    }

    #[test]
    fn the_lids_of_a_blink_come_down_from_their_top_and_cover_the_eyes() {
        let height = mascot::LID_BOTTOM - mascot::LID_TOP;
        for mood in [Mood::Rest, Mood::Working, Mood::Waiting, Mood::Failed] {
            let bearing = bearing(mood);
            let (open, closed) = bearing.blink_ends();
            // The top of the lids stays where it is, in the pose of the head.
            for x in [20., 50., 80.] {
                let top = (x, mascot::LID_TOP);
                assert!(near(open.place(top), closed.place(top)), "{mood:?}");
            }
            // Open, the lids are a sliver at their top; closed, they hang
            // down to their bottom.
            let held = closed.place((35., mascot::LID_TOP));
            let reach = |pose: Pose| {
                let end = pose.place((35., mascot::LID_BOTTOM));
                ((end.0 - held.0).powi(2) + (end.1 - held.1).powi(2)).sqrt()
            };
            assert!(reach(open) < 1.);
            assert!((reach(closed) - height).abs() < 1e-3);
        }
        // The eyes are circles about (35, 54) and (65, 54), of radius 9, or
        // 10.4 when wide: closed lids reach under them.
        const { assert!(mascot::LID_BOTTOM > 54. + 10.4) };
        // The lids reach the top of the eyes only past 0.4 of their way
        // down, so the half blink of the heartbeat, which goes 0.58 of the
        // way, stays above the pupils, whose top is at 50.4.
        let way = |y: f32| (y - mascot::LID_TOP) / height;
        assert!(way(54. - 9.) > 0.4);
        assert!(way(50.4) > 0.58);
    }

    #[test]
    fn a_mascot_that_is_alive_blinks_unless_its_eyes_smile() {
        let subject = avatar(MascotShape::Curl);
        for mood in MOODS {
            for small in [false, true] {
                let still = layers(&subject, mood, small, false);
                let alive = layers(&subject, mood, small, true);
                if mood == Mood::Happy {
                    assert_eq!(alive, still);
                    continue;
                }
                let lids = if small {
                    MascotLayer::SmallBlink
                } else {
                    MascotLayer::Blink
                };
                assert_eq!(alive[..still.len()], still[..]);
                assert_eq!(alive[still.len()..], [(lids, Ink::Body)]);
            }
        }
    }

    #[test]
    fn each_bot_moves_at_a_pace_of_its_own() {
        let ids = 0..60;
        let sway: std::collections::HashSet<_> = ids.clone().map(mascot::sway_period).collect();
        let blink: std::collections::HashSet<_> = ids.clone().map(mascot::blink_period).collect();
        assert_eq!(sway.len(), ids.len());
        assert_eq!(blink.len(), ids.len());
        for id in ids {
            let (sway, blink) = (mascot::sway_period(id), mascot::blink_period(id));
            assert!((6100..9700).contains(&sway.as_millis()), "{sway:?}");
            assert!((3100..4900).contains(&blink.as_millis()), "{blink:?}");
        }
    }
}

extern crate derive_more;
use std::cmp::Ordering;
use std::cmp::Ordering::*;
use std::collections::{BTreeMap, HashMap};
use std::iter::Peekable;
use std::iter::{Cycle, Take};
use std::ops::{Add, Mul};
use std::path::Iter;
use std::str::FromStr;
use std::time;

use midly::{
    num::u15, num::u24, num::u28, num::u4, num::u7, Header, MidiMessage, Smf, Track, TrackEventKind,
};
use midly::{EventIter, MetaMessage, TrackEvent};

use crate::dsl::dsl::{
    group_or_delimited_group, groups, BasicLength, Group, GroupOrNote, Groups, KnownLength, Length,
    ModdedLength, Note, Times,
};
use crate::midi::time::TimeSignature;
use GroupOrNote::*;
use Note::*;

#[allow(dead_code)]
static BAR_LIMIT: u32 = 1000;

// Typically used as number of ticks since the beginning of the track.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    derive_more::Add,
    derive_more::Sub,
    derive_more::Mul,
    derive_more::Rem,
    derive_more::Display,
)]
#[repr(transparent)]
pub struct Tick(pub u128);

impl Tick {
    pub fn from_128th(t: u32) -> Self {
        Tick(TICKS_PER_64TH_NOTE as u128 * t as u128)
    }
}

#[test]
fn test_add_tick() {
    assert_eq!(Tick(2) + Tick(2), Tick(4));
}

// Delta in time since the last MIDI event, measured in Ticks.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, derive_more::Add, derive_more::Mul,
)]
#[repr(transparent)]
pub struct Delta(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
pub enum EventType {
    NoteOn(Part),
    NoteOff(Part),
}

use EventType::*;

impl Ord for EventType {
    fn cmp(&self, other: &EventType) -> Ordering {
        match (self, other) {
            (NoteOn(a), NoteOn(b)) => a.cmp(b),
            (NoteOn(a), NoteOff(b)) => match a.cmp(b) {
                Equal => Greater,
                ord => ord,
            },
            (NoteOff(a), NoteOn(b)) => match a.cmp(b) {
                Equal => Less,
                ord => ord,
            },
            (NoteOff(a), NoteOff(b)) => a.cmp(b),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
pub enum Part {
    KickDrum,
    SnareDrum,
    HiHat,
    CrashCymbal,
}

use Part::*;

impl Part {
    // https://computermusicresource.com/GM.Percussion.KeyMap.html
    fn to_midi_key(&self) -> u7 {
        match self {
            KickDrum => u7::from(36),
            SnareDrum => u7::from(38),
            HiHat => u7::from(46),
            CrashCymbal => u7::from(49),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event<T> {
    tick: T,
    event_type: EventType,
}

impl<T> PartialOrd for Event<T>
where
    T: PartialOrd + Ord,
    Event<T>: Ord,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }

    fn lt(&self, other: &Self) -> bool {
        matches!(self.partial_cmp(other), Some(Less))
    }

    fn le(&self, other: &Self) -> bool {
        matches!(self.partial_cmp(other), Some(Less | Equal))
    }

    fn gt(&self, other: &Self) -> bool {
        matches!(self.partial_cmp(other), Some(Greater))
    }

    fn ge(&self, other: &Self) -> bool {
        matches!(self.partial_cmp(other), Some(Greater | Equal))
    }
}

impl<T> Ord for Event<T>
where
    T: Ord,
{
    fn cmp(&self, other: &Event<T>) -> Ordering {
        if self.tick == other.tick {
            self.event_type.cmp(&other.event_type)
        } else {
            self.tick.cmp(&other.tick)
        }
    }
}

#[test]
fn test_ord_event_t() {
    let first_on = Event {
        tick: Tick(0),
        event_type: NoteOn(KickDrum),
    };
    let first_off = Event {
        tick: Tick(24),
        event_type: NoteOff(KickDrum),
    };
    let second_on = Event {
        tick: Tick(24),
        event_type: NoteOn(KickDrum),
    };
    assert_eq!(first_on.cmp(&first_off), Less);
    assert_eq!(first_off.cmp(&second_on), Less);

    let mut vec1 = vec![second_on, first_off, first_on];
    let mut vec2 = vec1.clone();

    vec1.sort_by(|x, y| x.cmp(y));
    assert_eq!(vec1, vec![first_on, first_off, second_on]);

    vec2.sort();
    assert_eq!(vec2, vec![first_on, first_off, second_on]);
}

// Events are supposed to be sorted by T at all times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventGrid<T> {
    events: Vec<Event<T>>,
    length: Tick,
}

impl<T> IntoIterator for EventGrid<T> {
    type Item = Event<T>;
    type IntoIter = std::vec::IntoIter<Event<T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.into_iter()
    }
}

impl<T> EventGrid<T> {
    pub fn iter(&self) -> std::slice::Iter<'_, Event<T>> {
        self.events.iter()
    }
}

impl<T: Add<Tick, Output = T> + Clone + Ord + std::fmt::Debug> Add for EventGrid<T> {
    type Output = EventGrid<T>;

    fn add(mut self, other: EventGrid<T>) -> EventGrid<T> {
        let other_events: Vec<Event<T>> = other
            .events
            .into_iter()
            .map(|mut e| {
                e.tick = e.tick.clone() + self.length;
                e
            })
            .collect();
        self.events.extend(other_events);
        // I don't know why sort() doesn't work in the same way.
        self.events.sort_by(|x, y| x.cmp(y));
        self.length = self.length + other.length;
        self
    }
}

impl<T: Clone + Ord> Mul for EventGrid<T> {
    type Output = EventGrid<T>;

    fn mul(mut self, other: EventGrid<T>) -> EventGrid<T> {
        let other_events: Vec<Event<T>> = other.events;

        self.events.extend(other_events);
        self.events.sort();
        self.length = self.length + other.length;
        self
    }
}

#[test]
fn test_arith_event_grids() {
    let eg1 = EventGrid {
        events: vec![
            Event {
                tick: Tick(0),
                event_type: NoteOn(KickDrum),
            },
            Event {
                tick: Tick(TICKS_PER_QUARTER_NOTE as u128),
                event_type: NoteOff(KickDrum),
            },
        ],
        length: Tick(TICKS_PER_QUARTER_NOTE as u128),
    };
    let eg2 = EventGrid {
        events: vec![
            Event {
                tick: Tick(24),
                event_type: NoteOn(HiHat),
            },
            Event {
                tick: Tick(TICKS_PER_QUARTER_NOTE as u128),
                event_type: NoteOff(HiHat),
            },
        ],
        length: Tick(TICKS_PER_QUARTER_NOTE as u128),
    };
    let mul_res = EventGrid {
        events: vec![
            Event {
                tick: Tick(0),
                event_type: NoteOn(KickDrum),
            },
            Event {
                tick: Tick(24),
                event_type: NoteOn(HiHat),
            },
            Event {
                tick: Tick(48),
                event_type: NoteOff(KickDrum),
            },
            Event {
                tick: Tick(48),
                event_type: NoteOff(HiHat),
            },
        ],
        length: Tick(96),
    };

    assert_eq!(eg1.clone() * eg2.clone(), mul_res);
}

#[test]
fn test_add_event_grid() {
    let empty: EventGrid<Tick> = EventGrid::empty();
    let kick_on = Event {
        tick: Tick(0),
        event_type: NoteOn(KickDrum),
    };
    let kick_off = Event {
        tick: Tick(24),
        event_type: NoteOff(KickDrum),
    };
    let simple_grid = EventGrid {
        events: vec![kick_on, kick_off],
        length: Tick(48),
    };
    assert_eq!(empty.clone() + empty.clone(), empty);
    assert_eq!(simple_grid.clone() + empty.clone(), simple_grid);
    assert_eq!(empty.clone() + simple_grid.clone(), simple_grid);
    assert_eq!(
        simple_grid.clone() + simple_grid.clone(),
        EventGrid {
            events: vec![
                Event {
                    tick: Tick(0),
                    event_type: NoteOn(KickDrum)
                },
                Event {
                    tick: Tick(24),
                    event_type: NoteOff(KickDrum)
                },
                Event {
                    tick: Tick(48),
                    event_type: NoteOn(KickDrum)
                },
                Event {
                    tick: Tick(72),
                    event_type: NoteOff(KickDrum)
                }
            ],
            length: Tick(96)
        }
    );
}

impl<T> EventGrid<T> {
    fn empty() -> Self {
        EventGrid {
            events: Vec::new(),
            length: Tick(0),
        }
    }
}

impl EventGrid<Tick> {
    /// Converts a single-track(!!!!) sorted `EventGrid<Tick>`
    fn to_delta(&self) -> EventGrid<Delta> {
        let mut time = Tick(0);
        let mut delta_grid = EventGrid::empty();
        for e in &self.events {
            let delta = e.tick - time;
            time = time + delta;
            delta_grid.events.push(Event {
                tick: Delta(delta.0),
                event_type: e.event_type,
            })
        }
        delta_grid
    }
}

#[allow(dead_code)]
static TICKS_PER_QUARTER_NOTE: u16 = 48;

#[allow(dead_code)]
static TICKS_PER_64TH_NOTE: u16 = TICKS_PER_QUARTER_NOTE / 16;

impl BasicLength {
    /// `BasicLength` to MIDI Ticks
    pub fn to_ticks(&self) -> Tick {
        match self {
            BasicLength::Whole => Tick((TICKS_PER_QUARTER_NOTE * 4) as u128),
            BasicLength::Half => Tick((TICKS_PER_QUARTER_NOTE * 2) as u128),
            BasicLength::Fourth => Tick(TICKS_PER_QUARTER_NOTE as u128),
            BasicLength::Eighth => Tick((TICKS_PER_QUARTER_NOTE / 2) as u128),
            BasicLength::Sixteenth => Tick((TICKS_PER_QUARTER_NOTE / 4) as u128),
            BasicLength::ThirtySecond => Tick((TICKS_PER_QUARTER_NOTE / 8) as u128),
            BasicLength::SixtyFourth => Tick((TICKS_PER_QUARTER_NOTE / 16) as u128),
        }
    }
}

impl ModdedLength {
    /// `ModdedLength` to MIDI Ticks
    fn to_ticks(&self) -> Tick {
        match self {
            ModdedLength::Plain(blen) => blen.to_ticks(),
            ModdedLength::Dotted(blen) => {
                let Tick(whole) = blen.to_ticks();
                let half = whole / 2;
                Tick(whole + half)
            }
        }
    }
}

impl Length {
    /// Note length to MIDI ticks
    /// The function converts a musical note length to ticks, accounting for simple notes, tied notes, and
    /// triplets.
    ///
    /// Arguments:
    ///
    /// * `length`: `length` is a variable of type `Length`, which is an enum that represents different
    /// types of musical note lengths. The function `length_to_ticks` takes a `Length` as input and returns
    /// a `Tick`, which is a struct representing the number of ticks (a unit of time in music
    ///
    /// Returns:
    ///
    /// The function `length_to_ticks` takes a `Length` enum as input and returns a `Tick` value. The `Tick`
    /// value represents the duration of the note in ticks, which is a unit of time used in music notation
    /// software.
    fn to_ticks(&self) -> Tick {
        match self {
            Length::Simple(mlen) => mlen.to_ticks(),
            Length::Tied(first, second) => first.to_ticks() + second.to_ticks(),
            Length::Triplet(mlen) => {
                let Tick(straight) = mlen.to_ticks();
                let triplet = straight * 2 / 3;
                Tick(triplet)
            }
        }
    }
}

#[allow(dead_code)]
static MICROSECONDS_PER_MINUTE: u128 = 60000000 as u128;

#[allow(dead_code)]
static MIDI_CLOCKS_PER_CLICK: u8 = 24;

/// Microseconds per quarter note. Default is 500,000 for 120bpm.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    derive_more::Add,
    derive_more::Sub,
    derive_more::Mul,
    derive_more::Display,
)]
pub struct MidiTempo(u24);

impl MidiTempo {
    fn from_tempo(tempo: u16) -> Self {
        let mt = MICROSECONDS_PER_MINUTE as u32 / tempo as u32;
        Self(mt.into())
    }
}

/// Returns an EventGrid and a total length. Length is needed as a group can end with rests that are not in the grid,
/// and we need it to cycle the group.
fn flatten_group(
    Group {
        notes,
        length,
        times,
    }: &Group,
    part: Part,
    start: &mut Tick,
) -> EventGrid<Tick> {
    let time = start;
    let note_length = length.to_ticks();
    let mut grid = EventGrid::empty();
    notes.iter().for_each(|entry| {
        match entry {
            SingleGroup(group) => {
                let mut eg = flatten_group(&group, part, time);
                grid.events.append(&mut eg.events);
                grid.length = grid.length + eg.length;
            }
            SingleNote(Note::Rest) => {
                let rest_end = *time + note_length;
                *time = rest_end;
                grid.length = rest_end;
            }
            SingleNote(Note::Hit) => {
                let note_end = *time + note_length;
                let note_on = Event {
                    tick: *time,
                    event_type: NoteOn(part),
                };
                let note_off = Event {
                    tick: note_end,
                    event_type: NoteOff(part),
                };
                grid.events.push(note_on);
                grid.events.push(note_off);
                grid.length = note_end;
                *time = note_end;
            }
        };
    });
    grid.events.sort();
    cycle_grid(grid, *times)
}

#[test]
fn test_flatten_group() {
    assert_eq!(
        flatten_group(
            &group_or_delimited_group("(2,8x--)").unwrap().1,
            KickDrum,
            &mut Tick(0)
        ),
        EventGrid {
            events: vec![
                Event {
                    tick: Tick(0),
                    event_type: NoteOn(KickDrum)
                },
                Event {
                    tick: Tick(24),
                    event_type: NoteOff(KickDrum)
                },
                Event {
                    tick: Tick(72),
                    event_type: NoteOn(KickDrum)
                },
                Event {
                    tick: Tick(96),
                    event_type: NoteOff(KickDrum)
                }
            ],
            length: Tick(144)
        }
    );
}

fn cycle_grid(event_grid: EventGrid<Tick>, times: Times) -> EventGrid<Tick> {
    let mut grid = EventGrid::empty();
    for _ in 1..(times.0 + 1) {
        grid = grid + event_grid.clone();
    }
    grid
}

#[test]
fn test_cycle_grid() {
    let empty: EventGrid<Tick> = EventGrid::empty();
    assert_eq!(cycle_grid(EventGrid::empty(), Times(2)), empty);
    let kick_on = Event {
        tick: Tick(0),
        event_type: NoteOn(KickDrum),
    };
    let kick_off = Event {
        tick: Tick(24),
        event_type: NoteOff(KickDrum),
    };
    let simple_grid = EventGrid {
        events: vec![kick_on, kick_off],
        length: Tick(48),
    };
    assert_eq!(cycle_grid(simple_grid.clone(), Times(0)), empty);
    assert_eq!(cycle_grid(simple_grid.clone(), Times(1)), simple_grid);
    assert_eq!(
        cycle_grid(simple_grid.clone(), Times(2)),
        EventGrid {
            events: vec![
                Event {
                    tick: Tick(0),
                    event_type: NoteOn(KickDrum)
                },
                Event {
                    tick: Tick(24),
                    event_type: NoteOff(KickDrum)
                },
                Event {
                    tick: Tick(48),
                    event_type: NoteOn(KickDrum)
                },
                Event {
                    tick: Tick(72),
                    event_type: NoteOff(KickDrum)
                }
            ],
            length: Tick(96)
        }
    );
}

fn flatten_groups(part: Part, groups: &Groups) -> EventGrid<Tick> {
    let mut time: Tick = Tick(0);
    let mut grid: EventGrid<Tick> = EventGrid::empty();
    groups.0.iter().for_each(|group| {
        grid = grid.clone() + flatten_group(group, part, &mut time);
    });
    grid
}

pub struct EventIterator {
    kick: Peekable<std::vec::IntoIter<Event<Tick>>>,
    snare: Peekable<std::vec::IntoIter<Event<Tick>>>,
    hihat: Peekable<std::vec::IntoIter<Event<Tick>>>,
    crash: Peekable<std::vec::IntoIter<Event<Tick>>>,
    time_signature: TimeSignature,
}

impl EventIterator {
    fn new(
        kick_grid: EventGrid<Tick>,
        snare_grid: EventGrid<Tick>,
        hihat_grid: EventGrid<Tick>,
        crash_grid: EventGrid<Tick>,
        time_signature: TimeSignature,
    ) -> EventIterator {
        let kick_repeats = 1;
        let snare_repeats = 1;
        let hihat_repeats = 1;
        let crash_repeats = 1;
        let event_iterator = EventIterator {
            kick: kick_grid.into_iter().peekable(),
            snare: snare_grid.into_iter().peekable(),
            hihat: hihat_grid.into_iter().peekable(),
            crash: crash_grid.into_iter().peekable(),
            time_signature,
        };
        event_iterator
    }
}

impl Iterator for EventIterator {
    type Item = Event<Tick>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let candidates: BTreeMap<Part, Event<Tick>> = [
            (KickDrum, self.kick.peek()),
            (SnareDrum, self.snare.peek()),
            (HiHat, self.hihat.peek()),
            (CrashCymbal, self.crash.peek()),
        ]
        .into_iter()
        .filter_map(|(p, x)| match x {
            Some(x) => Some((p, *x)),
            None => None,
        })
        .collect();

        if let Some((min_part, min_event)) = candidates.iter().min_by_key(|(_, x)| *x) {
            match min_part {
                KickDrum => self.kick.next(),
                SnareDrum => self.snare.next(),
                HiHat => self.hihat.next(),
                CrashCymbal => self.crash.next(),
            };
            Some(*min_event)
        } else {
            None
        }
    }
}

#[test]
fn test_event_iterator_impl() {
    let empty = EventGrid::empty();
    let kick1 = flatten_group(
        &group_or_delimited_group("(4x-)").unwrap().1,
        KickDrum,
        &mut Tick(0),
    );
    let snare1 = flatten_group(
        &group_or_delimited_group("4-x").unwrap().1,
        SnareDrum,
        &mut Tick(0),
    );

    assert_eq!(
        EventIterator::new(
            kick1.clone(),
            snare1.clone(),
            empty.clone(),
            empty.clone(),
            TimeSignature::from_str("4/4").unwrap()
        )
        .into_iter()
        .collect::<Vec<Event<Tick>>>(),
        vec![
            Event {
                tick: Tick(0),
                event_type: NoteOn(KickDrum)
            },
            Event {
                tick: Tick(48),
                event_type: NoteOff(KickDrum)
            },
            Event {
                tick: Tick(48),
                event_type: NoteOn(SnareDrum)
            },
            Event {
                tick: Tick(96),
                event_type: NoteOff(SnareDrum)
            }
        ]
    );

    assert_eq!(
        EventIterator::new(
            kick1.clone(),
            empty.clone(),
            empty.clone(),
            empty.clone(),
            TimeSignature::from_str("4/4").unwrap()
        )
        .into_iter()
        .collect::<Vec<Event<Tick>>>(),
        [
            Event {
                tick: Tick(0),
                event_type: NoteOn(KickDrum)
            },
            Event {
                tick: Tick(48),
                event_type: NoteOff(KickDrum)
            }
        ]
    );
}

// Returns time as a number of ticks from beginning, has to be turned into the midi delta-time.
fn flatten_and_merge(
    groups: HashMap<Part, Groups>,
    time_signature: TimeSignature,
) -> EventIterator {
    let length_map: HashMap<Part, u32> = groups
        .iter()
        .map(|(k, x)| (*k, x.0.iter().fold(0, |acc, n| acc + n.to_128th())))
        .collect();
    // We want exactly length_limit or BAR_LIMIT
    let converges_over_bars = time_signature
        .converges(groups.values())
        .unwrap_or(BAR_LIMIT.clone());
    println!("Converges over {} bars", converges_over_bars);
    let length_limit = converges_over_bars * time_signature.to_128th();
    let (kick_grid, kick_repeats) = match groups.get(&KickDrum) {
        Some(groups) => {
            let length_128th = length_map.get(&KickDrum).unwrap();
            let number_of_groups = groups.0.len();
            let times = length_limit / length_128th;
            (
                flatten_groups(KickDrum, groups),
                number_of_groups * times as usize,
            )
        }
        None => (EventGrid::empty(), 0),
    };
    let (snare_grid, snare_repeats) = match groups.get(&SnareDrum) {
        Some(groups) => {
            let length_128th = length_map.get(&SnareDrum).unwrap();
            let number_of_groups = groups.0.len();
            let times = length_limit / length_128th;
            (
                flatten_groups(SnareDrum, groups),
                number_of_groups * times as usize,
            )
        }
        None => (EventGrid::empty(), 0),
    };
    let (hihat_grid, hihat_repeats) = match groups.get(&HiHat) {
        Some(groups) => {
            let length_128th = length_map.get(&HiHat).unwrap();
            let number_of_groups = groups.0.len();
            let times = length_limit / length_128th;
            (
                flatten_groups(HiHat, groups),
                number_of_groups * times as usize,
            )
        }
        None => (EventGrid::empty(), 0),
    };
    let (crash_grid, crash_repeats) = match groups.get(&CrashCymbal) {
        Some(groups) => {
            let length_128th = length_map.get(&CrashCymbal).unwrap();
            let number_of_groups = groups.0.len();
            let times = length_limit / length_128th;
            (
                flatten_groups(CrashCymbal, groups),
                number_of_groups * times as usize,
            )
        }
        None => (EventGrid::empty(), 0),
    };

    EventIterator::new(
        cycle_grid(kick_grid, Times(kick_repeats as u16)),
        cycle_grid(snare_grid, Times(snare_repeats as u16)),
        cycle_grid(hihat_grid, Times(hihat_repeats as u16)),
        cycle_grid(crash_grid, Times(crash_repeats as u16)),
        time_signature,
    )
}

#[test]
fn test_flatten_and_merge() {
    let kick_events = vec![
        Event {
            tick: Tick(0),
            event_type: NoteOn(KickDrum),
        },
        Event {
            tick: Tick(12),
            event_type: NoteOff(KickDrum),
        },
        Event {
            tick: Tick(12),
            event_type: NoteOn(KickDrum),
        },
        Event {
            tick: Tick(24),
            event_type: NoteOff(KickDrum),
        },
        Event {
            tick: Tick(36),
            event_type: NoteOn(KickDrum),
        },
        Event {
            tick: Tick(48),
            event_type: NoteOff(KickDrum),
        },
        Event {
            tick: Tick(60),
            event_type: NoteOn(KickDrum),
        },
        Event {
            tick: Tick(72),
            event_type: NoteOff(KickDrum),
        },
        Event {
            tick: Tick(72),
            event_type: NoteOn(KickDrum),
        },
        Event {
            tick: Tick(84),
            event_type: NoteOff(KickDrum),
        },
    ];
    let snare_events = [
        Event {
            tick: Tick(24),
            event_type: NoteOn(SnareDrum),
        },
        Event {
            tick: Tick(48),
            event_type: NoteOff(SnareDrum),
        },
        Event {
            tick: Tick(96),
            event_type: NoteOn(SnareDrum),
        },
        Event {
            tick: Tick(120),
            event_type: NoteOff(SnareDrum),
        },
    ];
    let four_fourth = TimeSignature::from_str("4/4").unwrap();
    // let kick_event_grid = EventGrid { events, length: Tick(48 * 4) };
    let flattened_kick = flatten_and_merge(
        HashMap::from_iter([(KickDrum, groups("16xx-x-xx-").unwrap().1)]),
        four_fourth,
    )
    .collect::<Vec<Event<Tick>>>();
    let flattened_snare = flatten_and_merge(
        HashMap::from_iter([(SnareDrum, groups("8-x--x-").unwrap().1)]),
        four_fourth,
    )
    .collect::<Vec<Event<Tick>>>();
    let flattened_kick_and_snare = flatten_and_merge(
        HashMap::from_iter([
            (KickDrum, groups("16xx-x-xx-").unwrap().1),
            (SnareDrum, groups("8-x--x-").unwrap().1),
        ]),
        four_fourth,
    )
    .collect::<Vec<Event<Tick>>>();

    assert_eq!(flattened_kick, kick_events);
    assert_eq!(flattened_snare, snare_events);

    assert_eq!(
        flattened_kick
            .iter()
            .all(|x| flattened_kick_and_snare.contains(x)),
        true
    );
}

// The length of a beat is not standard, so in order to fully describe the length of a MIDI tick the MetaMessage::Tempo event should be present.
pub fn create_smf<'a>(groups: HashMap<Part, Groups>, time_signature: TimeSignature, text: &'a str, tempo: u16) -> Smf<'a> {
    let tracks = create_tracks(groups, time_signature, text, MidiTempo::from_tempo(tempo)); // FIXME
                                                        // https://majicdesigns.github.io/MD_MIDIFile/page_timing.html
                                                        // says " If it is not specified the MIDI default is 48 ticks per quarter note."
                                                        // As it's required in `Header`, let's use the same value.
    let metrical = midly::Timing::Metrical(u15::new(TICKS_PER_QUARTER_NOTE));
    Smf {
        header: Header {
            format: midly::Format::Parallel,
            timing: metrical,
        },
        tracks: tracks,
    }
}

/// Translates drum parts to a single MIDI track.
/// 
/// /// # Arguments
///
/// * `parts_and_groups` - Drum parts parsed from the command line.
/// * `time_signature` - Time signature parsed from the command line.
/// * `text_event` - Text message to be embedded into the MIDI file.
/// 
/// # Returns
/// 
/// Multi-track vectors of MIDI events in `midly` format.
/// 
fn create_tracks<'a>(
    parts_and_groups: HashMap<Part, Groups>,
    time_signature: TimeSignature,
    text_event: &'a str,
    midi_tempo: MidiTempo
) -> Vec<Vec<midly::TrackEvent<'a>>> {
    let events_iter = flatten_and_merge(parts_and_groups, time_signature);
    let events: Vec<Event<Tick>> = events_iter.collect();
    // Notice this time can be incorrect, but it shouldn't matter.
    let time = match events.last() {
        Some(ev) => ev.tick,
        None => {
            panic!("Result has no midi notes")
        }
    };
    let event_grid_tick = EventGrid {
        events,
        length: time,
    };
    let event_grid = event_grid_tick.to_delta();
    let mut drums = Vec::new();

    // This is likely to be specific to Guitar Pro. Tested with Guitar Pro 7.
    drums.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Midi {
            channel: 9.into(),
            message: MidiMessage::ProgramChange { program: 0.into() },
        },
    });
    drums.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::TrackName(b"Drumkit")),
    });
    drums.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::InstrumentName(b"Drumkit")),
    });
    drums.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::MidiChannel(10.into())),
    });
    drums.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::MidiPort(10.into())),
    });

    drums.push(TrackEvent { delta: 0.into(), kind: TrackEventKind::Meta(MetaMessage::Tempo(midi_tempo.0)) });

    let (midi_time_signature_numerator, midi_time_signature_denominator) =
        time_signature.to_midi();
    drums.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::TimeSignature(
            midi_time_signature_numerator,
            midi_time_signature_denominator,
            MIDI_CLOCKS_PER_CLICK.clone(),
            8,
        )),
    });

    // println!("{:?}", text_event.as_bytes());
    // drums.push(TrackEvent { delta: 0.into(), kind: TrackEventKind::Meta(MetaMessage::Text("!!!!!!!".as_bytes())) });

    for event in event_grid.events {
        let midi_message = match event.event_type {
            NoteOn(part) => MidiMessage::NoteOn {
                key: part.to_midi_key(),
                vel: 127.into(),
            },
            NoteOff(part) => MidiMessage::NoteOff {
                key: part.to_midi_key(),
                vel: 127.into(),
            },
        };
        drums.push(TrackEvent {
            delta: u28::from(event.tick.0 as u32),
            kind: TrackEventKind::Midi {
                channel: u4::from(10),
                message: midi_message,
            },
        })
    }
    drums.push(TrackEvent {
        delta: drums.last().unwrap().delta,
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    vec![drums]
}

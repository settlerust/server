//! Offline engine arena for research: complete games between independent seat policies that
//! receive only their own redacted observation and participant-visible events. Randomness comes
//! from an explicit per-game seed so lineups can be compared on identical boards. This is a
//! development and diagnostic tier; it never replaces the authoritative protocol arena.
pub mod seats;
pub mod table;
pub use seats::{Seat, SeatSpec, build_seat};
pub use table::{GameRecord, RobberMove, SeatRecord, Table, TableSettings, play};

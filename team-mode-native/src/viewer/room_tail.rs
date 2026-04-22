use std::io;
use std::path::Path;

use super::member_log::read_tail_lines;

pub fn read_room_tail(path: impl AsRef<Path>, max_lines: usize) -> io::Result<Vec<String>> {
    read_tail_lines(path, max_lines)
}

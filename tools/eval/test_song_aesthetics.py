from song_aesthetics import RATE, segments


def test_segments_are_whole_ten_second_windows_five_seconds_apart():
    spans = segments(25 * RATE)
    assert spans == [(0, 10 * RATE), (5 * RATE, 15 * RATE), (10 * RATE, 20 * RATE), (15 * RATE, 25 * RATE)]


def test_a_song_shorter_than_one_segment_has_none():
    assert segments(10 * RATE - 1) == []
    assert segments(10 * RATE) == [(0, 10 * RATE)]

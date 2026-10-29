# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Regression tests for the browser table's selection accounting.

`DataModel.count_enabled_cells()` exists to return the same number as
`len(selection.indexes())` without calling `flags()` once per cell, and
`Table._on_selection_changed()` has to keep using it for every selection
change, not only the ones made with a modifier key held.
"""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from typing import cast
from unittest.mock import MagicMock, patch

import pytest

from aqt.browser.table import CellRow, ItemId
from aqt.browser.table.model import DataModel
from aqt.browser.table.table import Table
from aqt.qt import (
    QAbstractTableModel,
    QItemSelection,
    QItemSelectionRange,
    QModelIndex,
    Qt,
)

# (top, left, bottom, right) in the stub model's coordinates.
Block = tuple[int, int, int, int]

ROWS = 12
COLUMNS = 4


class StubModel(QAbstractTableModel):
    """Stand-in for `DataModel` holding only what the counting code reads.

    `flags()`, `get_cached_row()`, `get_item()` and `count_enabled_cells()` are
    the real implementations, so the fast count and the per-cell flags stay
    checked against each other. Only the item list and the row cache are set
    directly, instead of being filled from a collection.
    """

    def __init__(
        self,
        rows: int = ROWS,
        columns: int = COLUMNS,
        cached_disabled: Iterable[int] = (),
        cached_enabled: Iterable[int] = (),
    ) -> None:
        super().__init__()
        # Item ids deliberately differ from row numbers: the cache is keyed by
        # item, and indexing it by row would pass a test that uses both. Only
        # `aqt` is imported here, because `anki.cards` cannot be the first
        # `anki` module a process imports.
        self._items: Sequence[ItemId] = [
            cast("ItemId", 100 + row) for row in range(rows)
        ]
        self._columns = columns
        self._rows: dict[int, CellRow] = {}
        for row in cached_disabled:
            self._rows[self._items[row]] = CellRow.disabled(columns, "deleted")
        for row in cached_enabled:
            self._rows[self._items[row]] = CellRow.generic(columns, "text")

    # Qt plumbing

    def rowCount(self, parent: QModelIndex = QModelIndex()) -> int:
        return 0 if parent.isValid() else len(self._items)

    def columnCount(self, parent: QModelIndex = QModelIndex()) -> int:
        return 0 if parent.isValid() else self._columns

    def data(self, index: QModelIndex, role: int = Qt.ItemDataRole.DisplayRole) -> None:
        return None

    # The code under test

    get_item = DataModel.get_item
    get_cached_row = DataModel.get_cached_row
    flags = DataModel.flags
    count_enabled_cells = DataModel.count_enabled_cells


class CountingStubModel(StubModel):
    """Stub model that records how often `flags()` is consulted."""

    def __init__(self, *args: object, **kwargs: object) -> None:
        super().__init__(*args, **kwargs)
        self.flags_calls = 0

    def flags(self, index: QModelIndex) -> Qt.ItemFlag:
        self.flags_calls += 1
        return StubModel.flags(self, index)


def select(model: StubModel, *blocks: Block) -> QItemSelection:
    """Build a selection of the given blocks, in order and without merging.

    `QItemSelection.select()` appends a range rather than normalising against
    the ranges already there, which is what lets overlapping blocks count the
    cells they share more than once.
    """
    selection = QItemSelection()
    for top, left, bottom, right in blocks:
        selection.select(model.index(top, left), model.index(bottom, right))
    return selection


@pytest.fixture
def model(qapp: object) -> StubModel:
    return StubModel()


# count_enabled_cells() agrees with len(indexes())
######################################################################


@pytest.mark.parametrize(
    "blocks",
    [
        pytest.param((), id="empty"),
        pytest.param(((0, 0, 0, 0),), id="single-cell"),
        pytest.param(((3, 1, 3, 2),), id="part-of-one-row"),
        pytest.param(((0, 0, ROWS - 1, COLUMNS - 1),), id="everything"),
        pytest.param(((2, 0, 5, COLUMNS - 1),), id="whole-rows"),
        pytest.param(((0, 2, ROWS - 1, 2),), id="one-whole-column"),
        pytest.param(((1, 0, 2, 1), (7, 2, 9, 3)), id="two-disjoint-blocks"),
        pytest.param(
            ((0, 0, 1, 1), (4, 0, 4, 0), (6, 1, 9, 3), (11, 3, 11, 3)),
            id="four-disjoint-blocks",
        ),
        pytest.param(((1, 0, 4, 2), (3, 1, 6, 3)), id="overlapping-blocks"),
        pytest.param(((2, 1, 4, 3), (2, 1, 4, 3)), id="the-same-block-twice"),
    ],
)
def test_count_equals_the_number_of_indexes(
    model: StubModel, blocks: tuple[Block, ...]
) -> None:
    selection = select(model, *blocks)

    assert model.count_enabled_cells(selection) == len(selection.indexes())


@pytest.mark.parametrize(
    "cached_disabled",
    [
        pytest.param((), id="none-disabled"),
        pytest.param((0,), id="first-row"),
        pytest.param((ROWS - 1,), id="last-row"),
        pytest.param((3,), id="inside-the-selection"),
        pytest.param((2, 3, 4), id="a-run-of-rows"),
        pytest.param((0, 5, 11), id="scattered-rows"),
        pytest.param(tuple(range(ROWS)), id="every-row"),
    ],
)
def test_count_equals_the_number_of_indexes_with_disabled_rows(
    qapp: object, cached_disabled: tuple[int, ...]
) -> None:
    model = StubModel(cached_disabled=cached_disabled)
    selection = select(model, (0, 0, ROWS - 1, COLUMNS - 1), (3, 1, 6, 2))

    assert model.count_enabled_cells(selection) == len(selection.indexes())


def test_disabled_rows_are_left_out_of_the_count(qapp: object) -> None:
    model = StubModel(cached_disabled=(1, 2))
    everything = select(model, (0, 0, ROWS - 1, COLUMNS - 1))

    assert model.count_enabled_cells(everything) == (ROWS - 2) * COLUMNS


def test_a_disabled_row_outside_the_selection_does_not_change_the_count(
    qapp: object,
) -> None:
    """The count only skips disabled rows it actually covers.

    The set of disabled items is built from the whole cache, so a disabled row
    elsewhere in the table must not be subtracted from an unrelated block.
    """
    model = StubModel(cached_disabled=(9, 10, 11))
    block = select(model, (0, 0, 3, COLUMNS - 1))

    assert model.count_enabled_cells(block) == 4 * COLUMNS
    assert model.count_enabled_cells(block) == len(block.indexes())


def test_rows_that_are_cached_and_enabled_are_counted(qapp: object) -> None:
    model = StubModel(cached_enabled=(0, 1, 2), cached_disabled=(3,))
    everything = select(model, (0, 0, ROWS - 1, COLUMNS - 1))

    assert model.count_enabled_cells(everything) == (ROWS - 1) * COLUMNS
    assert model.count_enabled_cells(everything) == len(everything.indexes())


def test_an_uncached_row_is_counted(qapp: object) -> None:
    """A row is only skipped once the cache says it is disabled.

    `flags()` reports an uncached row as enabled and selectable rather than
    fetching it, so the count has to treat an uncached row the same way.
    """
    model = StubModel()
    assert model._rows == {}

    everything = select(model, (0, 0, ROWS - 1, COLUMNS - 1))

    assert model.count_enabled_cells(everything) == ROWS * COLUMNS
    assert model.count_enabled_cells(everything) == len(everything.indexes())


def test_overlapping_blocks_count_their_shared_cells_twice(qapp: object) -> None:
    model = StubModel()
    first = (1, 0, 4, 2)
    second = (3, 1, 6, 3)

    overlapping = select(model, first, second)
    separately = model.count_enabled_cells(
        select(model, first)
    ) + model.count_enabled_cells(select(model, second))

    assert model.count_enabled_cells(overlapping) == separately
    assert model.count_enabled_cells(overlapping) == len(overlapping.indexes())


def test_an_invalid_range_is_skipped(qapp: object) -> None:
    """An invalid range contributes nothing, as it does to `indexes()`.

    `select()` drops a range built from invalid indexes instead of appending
    it, so the range has to be appended directly to get one into the
    selection at all.
    """
    model = StubModel()
    selection = select(model, (2, 0, 4, COLUMNS - 1))
    selection.append(QItemSelectionRange())
    assert [selection[i].isValid() for i in range(len(selection))] == [True, False]

    assert model.count_enabled_cells(selection) == 3 * COLUMNS
    assert model.count_enabled_cells(selection) == len(selection.indexes())


def test_an_empty_selection_counts_zero(model: StubModel) -> None:
    assert model.count_enabled_cells(QItemSelection()) == 0


# count_enabled_cells() does not call flags()
######################################################################


def test_counting_does_not_call_flags(qapp: object) -> None:
    """The whole point of the method: no `flags()` call per cell.

    `QItemSelection.indexes()` asks the model for every cell's flags, which is
    a Python call per cell and froze the browser on a large selection. Counting
    the same cells must not consult `flags()` at all.
    """
    model = CountingStubModel(cached_disabled=(4,))
    everything = select(model, (0, 0, ROWS - 1, COLUMNS - 1))

    assert len(everything.indexes()) == (ROWS - 1) * COLUMNS
    assert model.flags_calls >= ROWS * COLUMNS

    model.flags_calls = 0
    assert model.count_enabled_cells(everything) == (ROWS - 1) * COLUMNS
    assert model.flags_calls == 0


def test_counting_a_large_selection_does_not_call_flags(qapp: object) -> None:
    model = CountingStubModel(rows=5000, columns=8, cached_disabled=(7, 4999))
    everything = select(model, (0, 0, 4999, 7))

    assert model.count_enabled_cells(everything) == (5000 - 2) * 8
    assert model.flags_calls == 0


# Table._on_selection_changed() always uses the incremental count
######################################################################


def make_table(len_selection: int = 0, len_columns: int = COLUMNS) -> MagicMock:
    table = MagicMock()
    table._len_selection = len_selection
    table._selected_rows = [MagicMock()]
    table._model.len_columns.return_value = len_columns
    return table


def on_selection_changed(table: MagicMock, added: int, removed: int) -> None:
    """Run the handler with the two counts the model would have returned."""
    table._model.count_enabled_cells.side_effect = [added, removed]
    Table._on_selection_changed(table, MagicMock(), MagicMock())


def test_selection_change_adds_the_rows_gained_and_drops_those_lost() -> None:
    table = make_table(len_selection=10)

    on_selection_changed(table, added=8 * COLUMNS, removed=3 * COLUMNS)

    assert table._len_selection == 10 + 8 - 3


def test_selection_change_invalidates_the_cached_rows_and_tells_the_browser() -> None:
    table = make_table(len_selection=4)

    on_selection_changed(table, added=COLUMNS, removed=0)

    assert table._selected_rows is None
    table.browser.on_all_or_selected_rows_changed.assert_called_once_with()


def test_selection_change_does_not_look_at_the_keyboard_modifiers() -> None:
    """A new selection has to go through the same incremental count.

    The handler used to take the incremental path only while Shift or Ctrl was
    held and fall back to `selectedRows()` otherwise, which meant a plain click
    or Select All paid the cost `count_enabled_cells()` was added to avoid.
    """
    table = make_table(len_selection=0)

    with patch("aqt.browser.table.table.KeyboardModifiersPressed") as modifiers:
        on_selection_changed(table, added=50000 * COLUMNS, removed=0)

    modifiers.assert_not_called()
    table._selection_model.assert_not_called()
    assert table._model.count_enabled_cells.call_count == 2
    assert table._len_selection == 50000


def test_selection_change_counts_both_the_selected_and_deselected_sides() -> None:
    table = make_table(len_selection=1)
    selected = MagicMock()
    deselected = MagicMock()
    table._model.count_enabled_cells.side_effect = [2 * COLUMNS, COLUMNS]

    Table._on_selection_changed(table, selected, deselected)

    counted = [call.args[0] for call in table._model.count_enabled_cells.call_args_list]
    assert counted == [selected, deselected]
    assert table._len_selection == 1 + 2 - 1


def test_deselecting_everything_brings_the_length_back_to_zero() -> None:
    table = make_table(len_selection=50000)

    on_selection_changed(table, added=0, removed=50000 * COLUMNS)

    assert table._len_selection == 0

"""Observed menu state shared by camera adapters and illustrated guides."""
from typing import Literal, TypedDict


ScreenKind = Literal['list', 'popup', 'window', 'checklist', 'full_page',
                     'editor', 'live_picker', 'grid']


class MenuRow(TypedDict, total=False):
    index: int
    value: int
    child_pointer: int
    identity: tuple[object, ...]
    current_value: int | None
    checked: bool | None
    menu_item: int
    model_item: int


class MenuScreen(TypedDict, total=False):
    owner: str
    screen_kind: ScreenKind
    identity: tuple[object, ...]
    pointer: int
    menu_id: int | None
    type_code: int | None
    setting_id: int | None
    parent_pointer: int | None
    owner_item: int
    model_item: int
    focus: Literal['rows', 'tabs', 'fields']
    cursor: int | None
    columns: int
    rows_per_page: int
    wraps_navigation: bool
    raw_items: list[MenuRow]
    item_statuses: list[int]
    visible_indices: list[int]
    selected_item: MenuRow | None


def menu_model(state: MenuScreen) -> MenuScreen:
    """Normalize one owner's observation without reading camera memory.

    Native pointer and key fields identify menu occurrences. Adapters supply
    the actual rows and values; guides navigate those observations through
    physical input. An unknown current value or checkbox state is None.
    """
    model: MenuScreen = dict(menu_id=None, type_code=None, setting_id=None,
        parent_pointer=None, owner='stock', screen_kind='list', focus='rows', columns=1)
    model.update(state)
    model.setdefault('identity', ('node', model['pointer']))
    rows = model['raw_items']
    for row in rows:
        row.setdefault('identity', (model['identity'], row['value']))
        row.setdefault('current_value', None)
        row.setdefault('checked', None)
    cursor = model['cursor']
    if 'visible_indices' not in model:
        visible = [row['index'] for row, status in zip(rows, model['item_statuses']) if status != 3]
        width = model.get('rows_per_page', 0)
        page = visible.index(cursor) // width if width and cursor in visible else 0
        model['visible_indices'] = visible[page * width:(page + 1) * width] if width else []
    model['selected_item'] = (rows[cursor].copy() if model['focus'] != 'tabs'
                              and cursor is not None and 0 <= cursor < len(rows) else None)
    return model

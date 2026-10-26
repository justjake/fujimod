"""Merge PyO3's binary-generated emulator declarations into the native stub."""

from __future__ import annotations

import ast
import sys
from collections.abc import Sequence
from pathlib import Path

TYPE_OVERRIDES = {
    (None, "clear_parse", "initial_path"): "dict[str, int] | None",
    (None, "clear_parse", "initial_fill"): "dict[str, int] | None",
    (
        None,
        "clear_parse",
        "return",
    ): "tuple[dict[str, object], dict[str, int], dict[str, int]]",
    (None, "clear_path_descriptor", "return"): "dict[str, object]",
    (None, "load_input_assets", "return"): "dict[str, bytes]",
    (None, "persistent_packet", "assets"): "dict[str, bytes]",
    (None, "ui_batch", "initial_path"): "dict[str, int] | None",
    (None, "ui_batch", "initial_fill"): "dict[str, int] | None",
    (None, "ui_initialize", "initial_path"): "dict[str, int] | None",
    (None, "ui_initialize", "initial_fill"): "dict[str, int] | None",
    (
        None,
        "ui_initialize",
        "return",
    ): "tuple[dict[str, object], dict[str, int], dict[str, int]]",
    (None, "ui_stream_draws", "initial"): "dict[str, int] | None",
    (None, "ui_stream_parse", "initial"): "dict[str, int] | None",
    (
        None,
        "ui_stream_parse",
        "return",
    ): "tuple[list[dict[str, object]], dict[str, int]]",
    (None, "verify_input_assets", "assets"): "dict[str, bytes]",
    ("Board", "__new__", "assets"): "dict[str, bytes]",
    ("Board", "__new__", "models"): "dict[str, object]",
    ("Board", "card_format", "files"): "dict[str, bytes]",
    ("Board", "function_watch_state", "return"): "dict[str, object] | None",
    ("CardMedium", "sparse_sectors", "return"): "dict[int, bytes]",
    ("UiBatch", "banks", "return"): "tuple[dict[str, int], dict[str, int]]",
    ("UiBatch", "draw_dicts", "return"): "list[dict[str, object]]",
    ("UiBatch", "__iter__", "return"): "Iterator[dict[str, object]]",
    ("UiBatch", "reads", "return"): "list[tuple[str, int | tuple[int, int, int], int]]",
    ("UiBatch", "render", "targets"): "dict[int, bytes]",
    ("UiBatch", "render", "textures"): "dict[tuple[int, int, int], bytes]",
    ("UiBatch", "render", "return"): "tuple[list[tuple[int, bytes]], int]",
    ("Xp4", "device_records", "return"): "list[dict[str, object]]",
    ("Xp4", "driver_guard_state", "return"): "dict[str, object] | None",
    ("Xp4", "function_watch_state", "return"): "dict[str, object] | None",
    ("Xp4", "interrupt_state", "return"): "list[dict[str, int | bool]]",
    ("Xp4", "scanout_frames", "return"): "list[dict[str, object]]",
    ("Xp4", "state", "return"): "dict[str, object]",
}


def _module_imports(
    modules: Sequence[ast.Module],
) -> tuple[list[ast.Import], list[ast.ImportFrom]]:
    imports: dict[str, ast.Import] = {}
    from_imports: dict[
        tuple[int, str | None], dict[tuple[str, str | None], ast.alias]
    ] = {}
    for module in modules:
        for statement in module.body:
            if isinstance(statement, ast.Import):
                for alias in statement.names:
                    imports[ast.unparse(alias)] = ast.Import(names=[alias])
            elif isinstance(statement, ast.ImportFrom):
                level = (
                    0
                    if statement.level == 1 and statement.module == "collections.abc"
                    else statement.level
                )
                key = (level, statement.module)
                aliases = from_imports.setdefault(key, {})
                for alias in statement.names:
                    if alias.name != "Incomplete":
                        aliases[(alias.name, alias.asname)] = alias

    ordered_imports = [imports[key] for key in sorted(imports)]
    ordered_from_imports = [
        ast.ImportFrom(
            module=module,
            names=[aliases[key] for key in sorted(aliases)],
            level=level,
        )
        for (level, module), aliases in sorted(
            from_imports.items(), key=lambda item: (item[0][0], item[0][1] or "")
        )
    ]
    for module, name in (
        ("collections.abc", "Callable"),
        ("collections.abc", "Iterator"),
        ("emulator.binding_types", "MemoryReader"),
    ):
        key = (1 if module == "emulator.binding_types" else 0, module)
        aliases = from_imports.setdefault(key, {})
        aliases[(name, None)] = ast.alias(name=name)
    ordered_from_imports = [
        ast.ImportFrom(
            module=module,
            names=[aliases[key] for key in sorted(aliases)],
            level=level,
        )
        for (level, module), aliases in sorted(
            from_imports.items(), key=lambda item: (item[0][0], item[0][1] or "")
        )
    ]
    return ordered_imports, ordered_from_imports


def _annotation(expression: ast.expr | None) -> ast.expr | None:
    if expression is None:
        return None
    if isinstance(expression, ast.Constant) and isinstance(expression.value, str):
        expression = _parse_expression(expression.value)
    visited = _AnnotationNames().visit(expression)
    if not isinstance(visited, ast.expr):
        raise TypeError("Stub annotation transform did not return an expression")
    return visited


def _parse_expression(source: str) -> ast.expr:
    expression = ast.parse(source, mode="eval")
    if not isinstance(expression, ast.Expression):
        raise TypeError("Expected one stub annotation expression")
    return expression.body


class _AnnotationNames(ast.NodeTransformer):
    def visit_Attribute(self, node: ast.Attribute) -> ast.expr:
        if ast.unparse(node) == "collections.abc.Callable":
            return ast.copy_location(ast.Name(id="Callable", ctx=ast.Load()), node)
        visited = self.generic_visit(node)
        if not isinstance(visited, ast.expr):
            raise TypeError("Stub attribute transform did not return an expression")
        return visited


def _correct_members(generated: ast.Module) -> None:
    for statement in generated.body:
        if isinstance(statement, ast.ClassDef):
            for member in statement.body:
                _correct_statement(member, statement.name)
        else:
            _correct_statement(statement, None)


def _correct_statement(statement: ast.stmt, owner: str | None) -> None:
    if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
        for argument in (
            *statement.args.posonlyargs,
            *statement.args.args,
            *statement.args.kwonlyargs,
        ):
            override = TYPE_OVERRIDES.get((owner, statement.name, argument.arg))
            if override is not None:
                argument.annotation = _parse_expression(override)
            else:
                argument.annotation = _annotation(argument.annotation)
        override = TYPE_OVERRIDES.get((owner, statement.name, "return"))
        if override is not None:
            statement.returns = _parse_expression(override)
        else:
            statement.returns = _annotation(statement.returns)
    elif isinstance(statement, ast.AnnAssign):
        annotation = _annotation(statement.annotation)
        if annotation is None:
            raise TypeError("Native stub attributes require annotations")
        statement.annotation = annotation


def _emulator_class(generated: ast.Module) -> ast.ClassDef:
    _correct_members(generated)
    native_classes = {
        statement.name
        for statement in generated.body
        if isinstance(statement, ast.ClassDef)
    }
    _qualify_members(generated, native_classes)
    members: list[ast.stmt] = [
        ast.ClassDef(
            name="NativeError",
            bases=[ast.Name(id="RuntimeError")],
            keywords=[],
            body=[ast.Expr(value=ast.Constant(value=Ellipsis))],
            decorator_list=[],
        ),
        ast.ClassDef(
            name="DeviceRefusal",
            bases=[ast.Name(id="Exception")],
            keywords=[],
            body=[ast.Expr(value=ast.Constant(value=Ellipsis))],
            decorator_list=[],
        ),
    ]
    for statement in generated.body:
        if isinstance(statement, (ast.Import, ast.ImportFrom)):
            continue
        if isinstance(statement, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "__all__"
            for target in statement.targets
        ):
            continue
        if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if statement.name == "__getattr__":
                continue
            statement.decorator_list.insert(0, ast.Name(id="staticmethod"))
        members.append(statement)
    for statement in members:
        for item in ast.walk(statement):
            if isinstance(item, ast.Name) and item.id == "Any":
                raise ValueError("the generated emulator stub contains an Any type")
            if isinstance(item, ast.Name) and item.id in {"dict", "list"}:
                parent_is_subscript = any(
                    isinstance(parent, ast.Subscript) and parent.value is item
                    for parent in ast.walk(statement)
                )
                if not parent_is_subscript:
                    raise ValueError(
                        f"the generated emulator stub has an unparameterized {item.id}"
                    )
    return ast.ClassDef(
        name="emulator",
        bases=[],
        keywords=[],
        body=members,
        decorator_list=[],
    )


def _qualify_members(
    statement: ast.stmt | ast.Module, native_classes: set[str]
) -> None:
    if isinstance(statement, (ast.Module, ast.ClassDef)):
        for member in statement.body:
            _qualify_members(member, native_classes)
    elif isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
        for argument in (
            *statement.args.posonlyargs,
            *statement.args.args,
            *statement.args.kwonlyargs,
        ):
            if argument.annotation is not None:
                argument.annotation = _qualify_native_types(
                    argument.annotation, native_classes
                )
        if statement.returns is not None:
            statement.returns = _qualify_native_types(statement.returns, native_classes)
    elif isinstance(statement, ast.AnnAssign):
        statement.annotation = _qualify_native_types(
            statement.annotation, native_classes
        )


def _qualify_native_types(annotation: ast.expr, native_classes: set[str]) -> ast.expr:
    class Qualify(ast.NodeTransformer):
        def visit_Name(self, node: ast.Name) -> ast.expr:
            if node.id in native_classes:
                return ast.copy_location(
                    ast.Attribute(
                        value=ast.Name(id="emulator", ctx=ast.Load()),
                        attr=node.id,
                        ctx=ast.Load(),
                    ),
                    node,
                )
            return node

    result = Qualify().visit(annotation)
    assert isinstance(result, ast.expr)
    return result


def merge(generated_path: Path, target_path: Path) -> None:
    generated = ast.parse(generated_path.read_text())
    target = ast.parse(target_path.read_text())
    _remove_docstrings(generated)
    emulator = _emulator_class(generated)
    imports, from_imports = _module_imports((target, generated))
    declarations = [
        statement
        for statement in target.body
        if not isinstance(statement, (ast.Import, ast.ImportFrom))
        and not (isinstance(statement, ast.ClassDef) and statement.name == "emulator")
    ]
    declarations.append(emulator)
    output = ast.Module(body=[*imports, *from_imports, *declarations], type_ignores=[])
    ast.fix_missing_locations(output)
    used_names = {
        node.id
        for statement in declarations
        for node in ast.walk(statement)
        if isinstance(node, ast.Name)
    }
    required_quoted_names = {"MemoryReader", "catalog", "firmware", "usb"}
    for statement in from_imports:
        statement.names = [
            alias
            for alias in statement.names
            if (alias.asname or alias.name.rsplit(".", 1)[-1]) in used_names
            or alias.name in required_quoted_names
        ]
    from_imports = [statement for statement in from_imports if statement.names]
    output.body = [*imports, *from_imports, *declarations]
    ast.fix_missing_locations(output)
    target_path.write_text(ast.unparse(output) + "\n")


def _remove_docstrings(
    statement: ast.Module | ast.ClassDef | ast.FunctionDef | ast.AsyncFunctionDef,
) -> None:
    if not isinstance(
        statement, ast.Module | ast.ClassDef | ast.FunctionDef | ast.AsyncFunctionDef
    ):
        return
    body = statement.body
    if (
        body
        and isinstance(body[0], ast.Expr)
        and isinstance(body[0].value, ast.Constant)
        and isinstance(body[0].value.value, str)
    ):
        body.pop(0)
    if not body and isinstance(
        statement, ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef
    ):
        body.append(ast.Expr(value=ast.Constant(value=Ellipsis)))
    for member in body:
        if isinstance(
            member, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
        ):
            _remove_docstrings(member)


def main(arguments: Sequence[str]) -> None:
    if len(arguments) != 2:
        raise SystemExit(
            "usage: merge_native_emulator_stub.py GENERATED_MODULE_STUB NATIVE_STUB"
        )
    merge(Path(arguments[0]), Path(arguments[1]))


if __name__ == "__main__":
    main(sys.argv[1:])

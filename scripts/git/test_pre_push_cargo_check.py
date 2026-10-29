from pathlib import Path

from pre_push_cargo_check import (
    is_rust_input,
    select_browser_wasm_packages,
    select_touched_crates,
)


def test_touched_rust_paths_select_only_their_workspace_crates(tmp_path: Path) -> None:
    packages = [
        {
            "name": "fujimod-core",
            "manifest_path": str(tmp_path / "crates/core/Cargo.toml"),
        },
        {
            "name": "fujimod-usb",
            "manifest_path": str(tmp_path / "crates/usb/Cargo.toml"),
        },
        {
            "name": "fujimod-web",
            "manifest_path": str(tmp_path / "bindings/web/Cargo.toml"),
        },
    ]

    selected = select_touched_crates(
        [
            "crates/core/src/lib.rs",
            "crates/usb/Cargo.toml",
            "crates/web/README.md",
            "README.md",
        ],
        packages,
        tmp_path,
    )

    assert [package["name"] for package in selected] == ["fujimod-core", "fujimod-usb"]
    assert is_rust_input("crates/core/src/lib.rs")
    assert not is_rust_input("README.md")


def test_workspace_rust_configuration_selects_every_crate(tmp_path: Path) -> None:
    packages = [
        {
            "name": "fujimod-core",
            "manifest_path": str(tmp_path / "crates/core/Cargo.toml"),
        },
        {
            "name": "fujimod-usb",
            "manifest_path": str(tmp_path / "crates/usb/Cargo.toml"),
        },
    ]

    selected = select_touched_crates(["Cargo.toml"], packages, tmp_path)

    assert [package["name"] for package in selected] == ["fujimod-core", "fujimod-usb"]


def test_workspace_manifest_change_does_not_wasm_check_unicorn_sys(
    tmp_path: Path,
) -> None:
    package_specs = [
        ("fujimod-javascript", "bindings/javascript"),
        ("fujimod-usb", "crates/usb"),
        ("fujimod-usb-proxy", "crates/usb-proxy"),
        ("fujimod-emulator-cpu-unicorn", "crates/emulator-cpu-unicorn"),
        ("fujimod-unicorn-sys", "crates/emulator-cpu-unicorn/sys"),
        ("fujimod-emulator-gpu", "crates/emulator-gpu"),
    ]
    packages = [
        {
            "id": package_name,
            "name": package_name,
            "manifest_path": str(tmp_path / package_path / "Cargo.toml"),
        }
        for package_name, package_path in package_specs
    ]
    ids = {package["name"]: package["id"] for package in packages}

    def dependency(package_name: str, kind: str | None = None) -> dict:
        return {
            "pkg": ids[package_name],
            "dep_kinds": [{"kind": kind, "target": None}],
        }

    metadata = {
        "workspace_members": [package["id"] for package in packages],
        "packages": packages,
        "resolve": {
            "nodes": [
                {
                    "id": ids["fujimod-javascript"],
                    "deps": [
                        dependency("fujimod-usb"),
                        dependency("fujimod-emulator-cpu-unicorn", "dev"),
                        dependency("fujimod-unicorn-sys", "build"),
                        dependency("fujimod-emulator-gpu", "build"),
                    ],
                },
                {
                    "id": ids["fujimod-usb"],
                    "deps": [dependency("fujimod-usb-proxy")],
                },
                {"id": ids["fujimod-usb-proxy"], "deps": []},
                {"id": ids["fujimod-emulator-cpu-unicorn"], "deps": []},
                {"id": ids["fujimod-unicorn-sys"], "deps": []},
                {"id": ids["fujimod-emulator-gpu"], "deps": []},
            ],
        },
    }

    native_packages = select_touched_crates(["Cargo.toml"], packages, tmp_path)
    wasm_packages = select_browser_wasm_packages(metadata, native_packages, tmp_path)

    assert [package["name"] for package in native_packages] == sorted(ids)
    assert [package["name"] for package in wasm_packages] == [
        "fujimod-javascript",
        "fujimod-usb",
        "fujimod-usb-proxy",
    ]

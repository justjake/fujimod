"""Built web app virtual SD insertion, downloads, retained power cycle and format.

Uses only visible controls and observes the existing worker protocol. No
firmware patches, guest-memory writes or physical camera access.
"""

import argparse
import asyncio
import hashlib
import json
import zipfile
from pathlib import Path

from interactions import segment, tap
from playwright.async_api import async_playwright


async def check(args: argparse.Namespace) -> None:
    args.output.mkdir(parents=True, exist_ok=False)
    async with async_playwright() as p:
        browser = await p.webkit.launch(headless=False)
        page = await browser.new_page(**p.devices["iPhone 13"])
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        await page.add_init_script("""const W=window.Worker;window.Worker=class extends W{
         constructor(...a){super(...a);this.addEventListener('message',({data})=>{
          if(data.state)window.__state=data.state;if(data.card)window.__card=data.card;
         });}};""")
        result = {}

        async def ready() -> None:
            await page.wait_for_function(
                'document.querySelector("#live-label").textContent==="LIVE" || document.querySelector("#status").classList.contains("error")',
                timeout=180000,
            )
            assert await page.locator("#live-label").inner_text() == "LIVE", (
                await page.locator("#fault-raw").inner_text()
            )

        async def download(name: str) -> dict[str, str]:
            async with page.expect_download(timeout=60000) as pending:
                await tap(page.locator("#card-download"))
            item = await pending.value
            target = args.output / name
            await item.save_as(target)
            with zipfile.ZipFile(target) as z:
                return {n: hashlib.sha256(z.read(n)).hexdigest() for n in z.namelist()}

        try:
            await page.goto(args.url)
            await ready()
            await page.wait_for_function(
                'document.querySelectorAll("#card-files .r").length>=2'
            )
            original = await download("original.zip")
            assert {n.lower() for n in original} == {"ffboot.bin", "ffsys.bin"}, (
                original
            )
            await tap(page.locator('[data-card-present="false"]'))
            await page.wait_for_function("window.__card?.present===false")
            assert await page.locator("#card-files .r").count() == 0
            await tap(page.locator('[data-card-present="true"]'))
            await page.wait_for_function(
                'window.__card?.present===true && document.querySelectorAll("#card-files .r").length>=2'
            )
            assert await download("reinserted.zip") == original
            await segment(page, "power", "off")
            await page.wait_for_function("window.__state?.power?.on===false")
            await segment(page, "power", "on")
            await page.wait_for_function(
                "window.__state?.power?.on===true", timeout=180000
            )
            await page.wait_for_function(
                'document.querySelector(".control-node[data-id=power] button[data-value=on]").disabled===false',
                timeout=180000,
            )
            assert await download("power-cycle.zip") == original
            await tap(page.locator("#card-format"))
            await page.wait_for_function(
                'document.querySelector("#live-label").textContent==="RESTARTING"'
            )
            await ready()
            await page.wait_for_function(
                'document.querySelectorAll("#card-files .r").length===0'
            )
            assert await page.locator("#card-capacity").inner_text() == "FAT32 · 8 GB"
            await page.screenshot(path=args.output / "formatted.png")
            assert not errors, errors
            result = dict(
                checks=[
                    "insertion",
                    "byte-identical reinsert",
                    "byte-identical power cycle",
                    "empty formatted 8 GiB card",
                ],
                seed_sha256=original,
                errors=errors,
            )
            print(json.dumps(result), flush=True)
        except BaseException:
            await page.screenshot(path=args.output / "failure.png")
            result["failure"] = await page.evaluate(
                '({state:window.__state,card:window.__card,error:document.querySelector("#fault-raw").textContent})'
            )
            raise
        finally:
            (args.output / "result.json").write_text(
                json.dumps(result, indent=2) + "\n"
            )
            await browser.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:5190/")
    parser.add_argument("--output", required=True, type=Path)
    asyncio.run(check(parser.parse_args()))

"""Check edge-to-edge mobile layout and real fault toast visibility on the preview."""

import argparse
import asyncio
from pathlib import Path
from typing import TypedDict, cast

from playwright.async_api import (
    Page,
    async_playwright,
)


class LayoutGeometry(TypedDict):
    stage: dict[str, float]
    viewer: dict[str, float]
    toast: dict[str, float]
    lcd: dict[str, float]
    radius: str
    overflow: bool
    toastOutside: bool


async def geometry(page: Page, width: int, height: int) -> LayoutGeometry:
    await page.set_viewport_size(dict(width=width, height=height))
    await page.evaluate("scrollTo(0, 0)")
    await page.wait_for_timeout(100)
    result = cast(
        LayoutGeometry,
        await page.evaluate("""() => {
      const stage = document.querySelector('#workspace');
      const viewer = document.querySelector('#viewer');
      const toast = document.querySelector('#engine-status');
      const lcd = document.querySelector('#lcd-source');
      const rect = element => {
        const r = element.getBoundingClientRect();
        return {left:r.left, right:r.right, top:r.top, bottom:r.bottom};
      };
      return {stage:rect(stage), viewer:rect(viewer), toast:rect(toast), lcd:rect(lcd),
        radius:getComputedStyle(stage).borderRadius,
        overflow:document.documentElement.scrollWidth > innerWidth,
        toastOutside:toast.parentElement === document.body};
    }"""),
    )
    if width <= 820:
        assert result["radius"] == "0px", result
        for element in ("stage", "viewer", "lcd"):
            assert abs(result[element]["left"]) < 1, result
            assert abs(result[element]["right"] - width) < 1, result
    assert result["toastOutside"], result
    if await page.locator("#engine-status").is_visible():
        toast = result["toast"]
        assert 0 <= toast["left"] < toast["right"] <= width, result
        assert 0 <= toast["top"] < toast["bottom"] <= height, result
        # Hit-test the real recovery controls, including after scrolling the stage away.
        for selector in ("#restart", "#error-report"):
            await page.locator(selector).scroll_into_view_if_needed()
            assert await page.locator(selector).evaluate("""element => {
              const r = element.getBoundingClientRect();
              return element.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));
            }"""), (selector, result)
    return result


async def check(args: argparse.Namespace) -> None:
    args.output.mkdir(parents=True, exist_ok=False)
    async with async_playwright() as playwright:
        for name in ("chromium", "webkit"):
            browser = await getattr(playwright, name).launch()
            try:
                page = await browser.new_page(viewport=dict(width=390, height=844))
                await page.goto(args.url)
                await page.wait_for_function(
                    "document.querySelector('#live-label').textContent === 'STOPPED'",
                    timeout=240000,
                )
                await page.wait_for_function(
                    "document.querySelector('#viewer').dataset.faultAnimation === 'settled'"
                )
                assert (
                    "SensorCaptureDMA" in await page.locator("#fault-raw").inner_text()
                )
                for width, height in (
                    (320, 568),
                    (390, 844),
                    (820, 900),
                    (821, 900),
                    (1300, 900),
                    (844, 390),
                ):
                    print(
                        name,
                        width,
                        height,
                        await geometry(page, width, height),
                        flush=True,
                    )
                    await page.screenshot(path=str(args.output / f"{name}-{width}.png"))
                    await page.evaluate("scrollTo(0, document.body.scrollHeight)")
                    await page.locator("#restart").click(trial=True)
                    await page.locator("#error-report").click(trial=True)
            finally:
                await browser.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--output", type=Path, required=True)
    asyncio.run(check(parser.parse_args()))

"""Verify one-click delivery and recovery after a real emulator execution fault.

The development upload marker and external GitHub navigation are substituted. window.open is made to navigate
the current tab, matching the embedded-browser behavior under test.
"""

import argparse
import asyncio
import hashlib
import json
import zipfile
from pathlib import Path
from urllib.parse import parse_qsl, urlparse

from playwright.async_api import async_playwright


async def check(args: argparse.Namespace) -> None:
    args.output.mkdir(parents=True, exist_ok=False)
    async with async_playwright() as p:
        for name in args.browser or ["chromium", "webkit"]:
            browser = await getattr(p, name).launch()
            context = await browser.new_context(viewport={"width": 390, "height": 844})
            await context.route(
                "https://github.com/justjake/fujimod/issues/new?*",
                lambda route: route.fulfill(body="GitHub issue destination"),
            )
            await context.add_init_script("""
              window.open = url => { location.assign(url); return null; };
            """)
            page = await context.new_page()
            # Preserve start-mode hydration, then disable development delivery
            # before real execution reaches the fault under test.
            await page.goto(args.url)
            await page.wait_for_function(
                "() => document.documentElement.dataset.reportsReady === 'true'"
            )
            await page.locator('meta[name="fujimod-local-reports"]').evaluate(
                "node => node.remove()"
            )
            await page.wait_for_function(
                "() => document.querySelector('#live-label').textContent === 'STOPPED'",
                timeout=240000,
            )
            assert (
                "Runtime did not initialize"
                not in await page.locator("#fault-raw").inner_text()
            )
            notification = [
                await page.locator(selector).inner_text()
                for selector in ["#fault-heading", "#fault-message", "#fault-raw"]
            ]
            await page.locator("#error-report").click()
            await page.locator("#feedback-description").fill(
                "Report delivery on iPhone"
            )
            downloads = []
            page.on(
                "download",
                lambda download, downloads=downloads: downloads.append(download),
            )
            await page.locator("#feedback-prepare").click()
            await page.wait_for_function(
                "() => location.hostname === 'github.com'", timeout=60000
            )
            assert len(context.pages) == 1
            assert "template=simulator-crash.yml" in page.url
            issue_url = page.url
            crash = dict(parse_qsl(urlparse(issue_url).query))["crash"]
            assert crash == "\n".join(text.strip() for text in notification)
            print(
                f"{name}: one-click delivery attempted; same-tab navigation reached GitHub",
                flush=True,
            )
            # A full reload must recover the saved bytes without regenerating them.
            await page.goto(args.url.split("?")[0])
            await page.locator('#viewer-tools[data-ready="true"]').wait_for(state="attached")
            await page.locator("#feedback-recover").wait_for(
                state="visible", timeout=30000
            )
            stored_hash = await page.evaluate("""async () => {
              const db = await new Promise((resolve,reject) => {
                const request=indexedDB.open('fujimod-feedback-v1',1);
                request.onsuccess=()=>resolve(request.result); request.onerror=()=>reject(request.error);
              });
              const report=await new Promise(resolve => {
                const request=db.transaction('reports').objectStore('reports').get('latest');
                request.onsuccess=()=>resolve(request.result);
              });
              db.close();
              const hash=await crypto.subtle.digest('SHA-256', report.bytes);
              return Array.from(new Uint8Array(hash), b=>b.toString(16).padStart(2,'0')).join('');
            }""")
            async with page.expect_download(timeout=60000) as retry:
                await page.locator("#feedback-recover").click()
            restored = args.output / f"{name}-restored.zip"
            await (await retry.value).save_as(restored)
            assert hashlib.sha256(restored.read_bytes()).hexdigest() == stored_hash
            with zipfile.ZipFile(restored) as archive:
                report = json.loads(archive.read("report.json"))
                assert report["description"] == "Report delivery on iPhone"
                assert report["issue"]["url"] == issue_url
                assert report["issue"]["fields"] == dict(
                    parse_qsl(urlparse(issue_url).query)
                )
                assert report["error"]["details"]["device"] == "SensorCaptureDMA"
                assert report["identity"]["firmwareSha256"]
                if "snapshot.zip" in archive.namelist():
                    assert archive.read("snapshot.zip")
                else:
                    assert report["snapshot"]["unavailable"]
            assert len(context.pages) == 1 and page.url == args.url.split("?")[0]
            print(
                f"{name}: saved ZIP survives same-tab navigation and reload; recovery downloads identical bytes",
                flush=True,
            )
            await browser.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--browser", action="append", choices=["chromium", "webkit"])
    asyncio.run(check(parser.parse_args()))

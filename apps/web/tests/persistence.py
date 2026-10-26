"""Exercise actual worker OPFS/IndexedDB round trips, without loading firmware."""

import argparse
import asyncio
import json
import subprocess
from pathlib import Path
from urllib.parse import urljoin

from playwright.async_api import Playwright, async_playwright


async def check(playwright: Playwright, args: argparse.Namespace, name: str) -> None:
    browser = await getattr(playwright, name).launch(headless=False)
    try:
        context = await browser.new_context()
        module = urljoin(args.url, "checkpoint-store.ts")
        source = Path(__file__).resolve().parents[1] / "src/engine/checkpoint-store.ts"
        compiled = subprocess.check_output(
            [
                "node",
                "--input-type=module",
                "-e",
                "import {readFileSync} from 'node:fs';import {stripTypeScriptTypes} from 'node:module';"
                "process.stdout.write(stripTypeScriptTypes(readFileSync(process.argv[1],'utf8')));",
                str(source),
            ],
            text=True,
        )
        await context.route(
            module,
            lambda route: route.fulfill(body=compiled, content_type="text/javascript"),
        )
        await context.route(
            args.url,
            lambda route: route.fulfill(
                body="<html><body>Worker persistence check</body></html>",
                content_type="text/html",
            ),
        )
        page = await context.new_page()
        await page.goto(args.url)
        result = await page.evaluate(
            """async module => {
          const source = `import {CheckpointStore} from ${JSON.stringify(module)};
            self.onmessage = async ({data}) => {
              try {
                if (data.fallback) Object.defineProperty(navigator.storage, 'getDirectory', {
                  value: () => Promise.reject(Error('OPFS disabled for fallback check'))
                });
                const store = new CheckpointStore();
                if (data.action === 'write') {
                  const saved = await store.write(data.key, new Uint8Array([0,1,128,255]));
                  self.postMessage({saved, backend: store.backend});
                } else {
                  const bytes = await store.read(data.key);
                  self.postMessage({bytes: bytes && Array.from(new Uint8Array(bytes)), backend: store.backend});
                }
              } catch (error) { self.postMessage({error: String(error)}); }
            };`;
          const url = URL.createObjectURL(new Blob([source], {type:'text/javascript'}));
          async function run(data) {
            const worker = new Worker(url, {type:'module'});
            try {
              return await new Promise((resolve,reject) => {
                const timer = setTimeout(() => reject(Error('Worker persistence timed out')), 10000);
                worker.onerror = event => { clearTimeout(timer); reject(Error(event.message)); };
                worker.onmessage = ({data}) => { clearTimeout(timer); resolve(data); };
                worker.postMessage(data);
              });
            } finally { worker.terminate(); }
          }
          try {
            const results = [];
            for (const fallback of [false,true]) {
              const key = `browser-test-${fallback}`;
              const write = await run({action:'write', key, fallback});
              // A fresh worker proves persistence survives runtime teardown.
              const read = await run({action:'read', key, fallback});
              results.push({fallback, write, read});
            }
            return results;
          } finally { URL.revokeObjectURL(url); }
        }""",
            module,
        )
        for item in result:
            assert item["write"].get("saved") is True, item
            assert item["read"].get("bytes") == [0, 1, 128, 255], item
            assert item["write"]["backend"] == item["read"]["backend"], item
            if item["fallback"]:
                assert item["write"]["backend"] == "indexeddb", item
        (args.output / f"{name}.json").write_text(json.dumps(result, indent=2) + "\n")
        print(name, json.dumps(result), flush=True)
    finally:
        await browser.close()


async def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:5190/")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--engines", nargs="+", default=["chromium", "webkit"])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    async with async_playwright() as playwright:
        for name in args.engines:
            await check(playwright, args, name)


if __name__ == "__main__":
    asyncio.run(main())

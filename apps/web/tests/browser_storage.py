"""Explicit cold-test setup, confined to this app's Playwright origin storage."""

from urllib.parse import urljoin

from playwright.async_api import Page


async def clear_camera_storage(page: Page, url: str) -> None:
    helper = urljoin(url, "/__fujimod_test_storage")
    await page.route(
        helper,
        lambda route: route.fulfill(body="<title>Cold test storage reset</title>"),
    )
    try:
        await page.goto(helper)
        await page.evaluate("""async()=>{
          const names=['fujimod-checkpoints-v1','fujimod-assets-v1'];
          try {
            const root=await navigator.storage.getDirectory();
            for(const name of names)await root.removeEntry(name,{recursive:true}).catch(()=>{});
          } catch {}
          for(const name of names)await new Promise((resolve,reject)=>{
            const request=indexedDB.deleteDatabase(name);
            request.onsuccess=()=>resolve();request.onerror=()=>reject(request.error);
            request.onblocked=()=>reject(Error('Another test still has this app database open'));
          });
        }""")
    finally:
        await page.unroute(helper)

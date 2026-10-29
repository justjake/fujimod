"""Rust runtime installer with an awaitable, caller-supplied Body.

Body methods: allocate(size, align) -> base; free(base); read(address, size)
-> bytes; write(address, data); synchronize(address, size); protect(address,
size); control(entry, operation); placed(image, lease). The placed notification
lets callers persist the image and allocation before the first upload.
"""
import json

from . import usb


class RuntimeInstallError(RuntimeError):
    """The installer retains its lease, including uncertain publication."""


class Installer:
    def __init__(self, image, body, *, lease=None, relink=None):
        self.image, self.body, self.lease = image, body, lease
        self.relink = relink

    @classmethod
    def plan(cls, data, metadata, body, *, relink=None):
        return cls(usb.RuntimeImage(data, json.dumps(metadata)), body, relink=relink)

    async def _run(self, operation, **arguments):
        task = usb.RuntimeTask(self.image, operation,
                               lease=json.dumps(self.lease) if self.lease else None,
                               relink=self.relink, **arguments)
        response = failure = None
        cause = None
        while True:
            kind, metadata, data = task.step(response, failure)
            facts = json.loads(metadata)
            response = failure = None
            if kind == 'done':
                self.lease = facts['lease']
                if operation != 'read':
                    self.image = usb.RuntimeImage(data, json.dumps(facts['image']))
                if 'error' in facts:
                    raise RuntimeInstallError(facts['error']) from cause
                return bytes(data) if operation == 'read' else facts['output']
            try:
                if kind == 'allocate':
                    base = await self.body.allocate(facts['size'], facts['align'])
                    response = base.to_bytes(8, 'little')
                elif kind == 'placed':
                    self.image = usb.RuntimeImage(data, json.dumps(facts['image']))
                    self.lease = facts['lease']
                    await self.body.placed(self.image, self.lease)
                elif kind == 'free':
                    await self.body.free(facts['base'])
                elif kind == 'read':
                    response = bytes(await self.body.read(facts['address'], facts['size']))
                elif kind == 'write':
                    await self.body.write(facts['address'], bytes(data))
                elif kind in ('synchronize', 'protect'):
                    await getattr(self.body, kind)(facts['address'], facts['size'])
                elif kind == 'control':
                    image = json.loads(self.image.metadata)
                    if facts['operation'] == image['control']['operations']['install']:
                        self.lease['publication_attempted'] = True
                    await self.body.control(facts['entry'], facts['operation'])
                else:
                    raise RuntimeError(f'Unknown Rust body operation: {kind}')
            except Exception as error:
                cause, failure = error, str(error)

    async def prepare(self):
        return await self._run('prepare')

    async def install(self, token=1):
        return await self._run('install', token=token)

    async def publish(self, token=1):
        return await self._run('publish', token=token)

    async def init(self, token=1):
        return await self._run('init', token=token)

    async def status(self, token=1):
        return await self._run('status', token=token)

    async def rollback(self, token=1):
        return await self._run('rollback', token=token)

    async def set(self, address, data):
        return await self._run('set', address=address, data=bytes(data))

    async def read(self, address, size):
        return await self._run('read', address=address, size=size)

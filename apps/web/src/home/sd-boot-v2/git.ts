/** A Git repository with one commit of the given files, as the files of its `.git` directory. */
export interface GitFile {
  /** Path inside the work tree, `/`-separated. */
  path: string;
  data: Uint8Array;
}
export interface GitCommit {
  name: string;
  email: string;
  message: string;
  /** Commit time in seconds since the epoch. */
  time: number;
}

const encoder = new TextEncoder();

async function sha1(data: Uint8Array) {
  return new Uint8Array(await crypto.subtle.digest('SHA-1', data.slice()));
}
const hex = (bytes: Uint8Array) =>
  Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');

function concat(parts: readonly Uint8Array[]) {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

async function zlib(data: Uint8Array) {
  const stream = new Blob([data.slice()]).stream().pipeThrough(new CompressionStream('deflate'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

interface Tree {
  files: Map<string, Uint8Array>;
  trees: Map<string, Tree>;
}

/** Git orders tree entries by name, comparing a directory as its name followed by `/`. */
function treeOrder(a: string, b: string) {
  const x = encoder.encode(a),
    y = encoder.encode(b);
  for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] - y[i];
  return x.length - y.length;
}

export async function gitRepository(
  files: readonly GitFile[],
  commit: GitCommit,
  branch = 'main',
): Promise<GitFile[]> {
  const objects = new Map<string, Uint8Array>();
  async function store(type: string, body: Uint8Array) {
    const object = concat([encoder.encode(`${type} ${body.length}\0`), body]);
    const id = await sha1(object);
    const name = hex(id);
    if (!objects.has(name)) objects.set(name, await zlib(object));
    return id;
  }
  const root: Tree = { files: new Map(), trees: new Map() };
  const blobs = new Map<string, { id: Uint8Array; size: number }>();
  for (const file of files) {
    const parts = file.path.split('/');
    if (parts.some((part) => !part || part === '.' || part === '..' || part === '.git'))
      throw Error(`Git cannot track ${file.path}.`);
    let tree = root;
    for (const part of parts.slice(0, -1)) {
      let next = tree.trees.get(part);
      if (!next) tree.trees.set(part, (next = { files: new Map(), trees: new Map() }));
      tree = next;
    }
    const id = await store('blob', file.data);
    tree.files.set(parts.at(-1)!, id);
    blobs.set(file.path, { id, size: file.data.length });
  }
  async function writeTree(tree: Tree): Promise<Uint8Array> {
    const entries: { key: string; mode: string; name: string; id: Uint8Array }[] = [];
    for (const [name, id] of tree.files) entries.push({ key: name, mode: '100644', name, id });
    for (const [name, child] of tree.trees)
      entries.push({ key: `${name}/`, mode: '40000', name, id: await writeTree(child) });
    entries.sort((a, b) => treeOrder(a.key, b.key));
    return store(
      'tree',
      concat(entries.flatMap((e) => [encoder.encode(`${e.mode} ${e.name}\0`), e.id])),
    );
  }
  const tree = await writeTree(root);
  const signature = `${commit.name} <${commit.email}> ${Math.floor(commit.time)} +0000`;
  const head = hex(
    await store(
      'commit',
      encoder.encode(
        `tree ${hex(tree)}\nauthor ${signature}\ncommitter ${signature}\n\n${commit.message.trim()}\n`,
      ),
    ),
  );
  const text = (path: string, value: string) => ({ path, data: encoder.encode(value) });
  return [
    text('HEAD', `ref: refs/heads/${branch}\n`),
    // FAT cards keep no execute bits or symbolic links and ignore case.
    text(
      'config',
      '[core]\n\trepositoryformatversion = 0\n\tfilemode = false\n\tbare = false\n\tsymlinks = false\n\tignorecase = true\n',
    ),
    text(`refs/heads/${branch}`, `${head}\n`),
    { path: 'index', data: await gitIndex(blobs) },
    ...Array.from(objects, ([name, data]) => ({
      path: `objects/${name.slice(0, 2)}/${name.slice(2)}`,
      data,
    })),
  ];
}

/** Index version 2 with zero stat data, so `git status` rehashes each file once and finds it clean. */
async function gitIndex(blobs: Map<string, { id: Uint8Array; size: number }>) {
  const paths = [...blobs.keys()].sort(treeOrder);
  const entries = paths.map((path) => {
    const name = encoder.encode(path);
    const length = Math.ceil((62 + name.length + 1) / 8) * 8;
    const entry = new Uint8Array(length),
      view = new DataView(entry.buffer);
    const blob = blobs.get(path)!;
    view.setUint32(24, 0o100644);
    view.setUint32(36, blob.size >>> 0);
    entry.set(blob.id, 40);
    view.setUint16(60, Math.min(name.length, 0xfff));
    entry.set(name, 62);
    return entry;
  });
  const header = new Uint8Array(12),
    view = new DataView(header.buffer);
  header.set(encoder.encode('DIRC'));
  view.setUint32(4, 2);
  view.setUint32(8, entries.length);
  const body = concat([header, ...entries]);
  return concat([body, await sha1(body)]);
}

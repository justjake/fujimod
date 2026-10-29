import { For } from 'solid-js';

/** Deliberately limited Markdown: text never becomes HTML or an image. */
function Inline(props: { text: string; depth?: number }) {
  const parts = () =>
    (props.depth ?? 0) > 8
      ? [props.text]
      : props.text.split(/(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|!?\[[^\]\n]+\]\([^\s)]+\))/g);
  return (
    <For each={parts()}>
      {(part) => {
        if (part.startsWith('`') && part.endsWith('`')) return <code>{part.slice(1, -1)}</code>;
        if (part.startsWith('**') && part.endsWith('**'))
          return (
            <strong>
              <Inline text={part.slice(2, -2)} depth={(props.depth ?? 0) + 1} />
            </strong>
          );
        if (part.startsWith('*') && part.endsWith('*'))
          return (
            <em>
              <Inline text={part.slice(1, -1)} depth={(props.depth ?? 0) + 1} />
            </em>
          );
        const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(part);
        if (link && /^(https?:\/\/|\/[^/]|#)/i.test(link[2]))
          return (
            <a href={link[2]} target="_blank" rel="noopener noreferrer">
              <Inline text={link[1]} depth={(props.depth ?? 0) + 1} />
            </a>
          );
        return part;
      }}
    </For>
  );
}
type Block =
  | { kind: 'heading' | 'paragraph' | 'code'; text: string }
  | { kind: 'list'; ordered: boolean; items: string[] };
export function noteBlocks(text: string): Block[] {
  const blocks: Block[] = [];
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (!line) continue;
    if (line.startsWith('```')) {
      const code: string[] = [];
      while (++i < lines.length && !lines[i].trim().startsWith('```')) code.push(lines[i]);
      blocks.push({ kind: 'code', text: code.join('\n') });
    } else if (/^#{1,6}\s/.test(line)) {
      blocks.push({ kind: 'heading', text: line.replace(/^#{1,6}\s+/, '') });
    } else {
      const item = /^(?:[-*+] |\d+\. )(.*)/.exec(line);
      const previous = blocks.at(-1);
      const ordered = /^\d+\./.test(line);
      if (item) {
        if (previous?.kind === 'list' && previous.ordered === ordered) previous.items.push(item[1]);
        else blocks.push({ kind: 'list', ordered, items: [item[1]] });
      } else if (
        previous?.kind === 'list' &&
        /^\s{2,}\S/.test(lines[i]) &&
        i > 0 &&
        lines[i - 1].trim()
      )
        previous.items[previous.items.length - 1] += ' ' + line;
      else if (previous?.kind === 'paragraph' && i > 0 && lines[i - 1].trim())
        previous.text += ' ' + line;
      else blocks.push({ kind: 'paragraph', text: line });
    }
  }
  return blocks;
}
export function ReleaseNotes(props: { text: string }) {
  return (
    <div class="release-notes">
      <For each={noteBlocks(props.text)}>
        {(block) => {
          switch (block.kind) {
            case 'heading':
              return (
                <h4>
                  <Inline text={block.text} />
                </h4>
              );
            case 'paragraph':
              return (
                <p>
                  <Inline text={block.text} />
                </p>
              );
            case 'code':
              return (
                <pre>
                  <code>{block.text}</code>
                </pre>
              );
            case 'list':
              return block.ordered ? (
                <ol>
                  <For each={block.items}>
                    {(item) => (
                      <li>
                        <Inline text={item} />
                      </li>
                    )}
                  </For>
                </ol>
              ) : (
                <ul>
                  <For each={block.items}>
                    {(item) => (
                      <li>
                        <Inline text={item} />
                      </li>
                    )}
                  </For>
                </ul>
              );
          }
        }}
      </For>
    </div>
  );
}

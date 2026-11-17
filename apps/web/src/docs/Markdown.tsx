import { For } from 'solid-js';
import type { JSX } from '@solidjs/web';

const KEYWORDS = new Set([
  'break', 'case', 'const', 'continue', 'default', 'do', 'else', 'enum', 'extern',
  'for', 'goto', 'if', 'inline', 'return', 'sizeof', 'static', 'struct', 'switch',
  'typedef', 'union', 'volatile', 'while',
]);
const TYPES = new Set([
  'bool', 'char', 'double', 'float', 'int', 'long', 'short', 'signed', 'size_t',
  'uint8_t', 'uint16_t', 'uint32_t', 'uint64_t', 'unsigned', 'void',
  'Clay_Color', 'Clay_String', 'Clay_StringSlice',
]);

const TOKEN = /\/\/[^\n]*|\/\*[\s\S]*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|^[ \t]*#[^\n]*|\b(?:0x[0-9a-fA-F]+|\d+(?:\.\d+)?)\b|\b[A-Za-z_]\w*\b/gm;

/** Tokenize C so documentation code can be read without an HTML pipeline. */
export function highlightC(source: string): JSX.Element[] {
  const output: JSX.Element[] = [];
  let last = 0;
  for (const match of source.matchAll(TOKEN)) {
    const index = match.index ?? 0;
    if (index > last) output.push(source.slice(last, index));
    const token = match[0];
    let kind = '';
    if (token.startsWith('//') || token.startsWith('/*')) kind = 'tok-comment';
    else if (token.startsWith('"') || token.startsWith("'")) kind = 'tok-string';
    else if (/^\s*#/.test(token)) kind = 'tok-preprocessor';
    else if (/^\d/.test(token)) kind = 'tok-number';
    else if (KEYWORDS.has(token)) kind = 'tok-keyword';
    else if (TYPES.has(token)) kind = 'tok-type';
    else if (token.startsWith('FOCAL_') || /^[A-Z][A-Z0-9_]+$/.test(token)) kind = 'tok-macro';
    output.push(kind ? <span class={kind}>{token}</span> : token);
    last = index + token.length;
  }
  if (last < source.length) output.push(source.slice(last));
  return output;
}

function Inline(props: { text: string; depth?: number }) {
  const parts = () =>
    (props.depth ?? 0) > 8
      ? [props.text]
      : props.text.split(/(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|!?\[[^\]\n]+\]\([^\s)]+\))/g);
  return (
    <For each={parts()}>
      {(part) => {
        if (part.startsWith('`') && part.endsWith('`'))
          return <code class="sdk-inline-code">{part.slice(1, -1)}</code>;
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
        if (link && /^(https?:\/\/|\/|#)/i.test(link[2]))
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
  | { kind: 'heading' | 'paragraph'; text: string }
  | { kind: 'code'; text: string; lang: string }
  | { kind: 'list'; ordered: boolean; items: string[] };

function blocks(text: string): Block[] {
  const result: Block[] = [];
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (!line) continue;
    if (line.startsWith('```')) {
      const lang = line.slice(3).trim();
      const code: string[] = [];
      while (++i < lines.length && !lines[i].trim().startsWith('```')) code.push(lines[i]);
      result.push({ kind: 'code', text: code.join('\n'), lang });
    } else if (/^#{1,6}\s/.test(line)) {
      result.push({ kind: 'heading', text: line.replace(/^#{1,6}\s+/, '') });
    } else {
      const item = /^(?:[-*+] |\d+\. )(.*)/.exec(line);
      const previous = result.at(-1);
      const ordered = /^\d+\./.test(line);
      if (item) {
        if (previous?.kind === 'list' && previous.ordered === ordered) previous.items.push(item[1]);
        else result.push({ kind: 'list', ordered, items: [item[1]] });
      } else if (previous?.kind === 'list' && /^\s{2,}\S/.test(lines[i]))
        previous.items[previous.items.length - 1] += ' ' + line;
      else if (previous?.kind === 'paragraph' && lines[i - 1]?.trim())
        previous.text += ' ' + line;
      else result.push({ kind: 'paragraph', text: line });
    }
  }
  return result;
}

function Code(props: { text: string; lang?: string }) {
  return (
    <pre class="sdk-code">
      <code>{props.lang === 'text' || props.lang === 'sh' ? props.text : highlightC(props.text)}</code>
    </pre>
  );
}

/** The subset of Markdown the component comments use. */
export function Markdown(props: { text: string; inline?: boolean }) {
  if (props.inline) return <Inline text={props.text} />;
  return (
    <div class="sdk-markdown">
      <For each={blocks(props.text)}>
        {(block) => {
          switch (block.kind) {
            case 'heading':
              return (
                <h3>
                  <Inline text={block.text} />
                </h3>
              );
            case 'paragraph':
              return (
                <p>
                  <Inline text={block.text} />
                </p>
              );
            case 'code':
              return <Code text={block.text} lang={block.lang} />;
            case 'list':
              return block.ordered ? (
                <ol>
                  <For each={block.items}>{(item) => <li><Inline text={item} /></li>}</For>
                </ol>
              ) : (
                <ul>
                  <For each={block.items}>{(item) => <li><Inline text={item} /></li>}</For>
                </ul>
              );
          }
        }}
      </For>
    </div>
  );
}

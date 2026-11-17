import { For } from 'solid-js';
import type { JSX } from '@solidjs/web';
import './steps.css';

export function Steps(props: { children: JSX.Element }) {
  return <ol class="recovery-steps">{props.children}</ol>;
}
export function Step(props: { title: string; pending?: boolean; children?: JSX.Element }) {
  return (
    <li class={{ 'recovery-pending': props.pending }}>
      <h2>{props.title}</h2>
      {props.children}
    </li>
  );
}
export function FileList(props: { files: readonly string[] }) {
  return (
    <For each={props.files}>
      {(name, i) => (
        <span>
          {i() ? ', ' : ''}
          <code>{name}</code>
        </span>
      )}
    </For>
  );
}

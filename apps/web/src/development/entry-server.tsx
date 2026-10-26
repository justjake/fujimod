/// <reference types="@solidjs/vite-plugin/virtual-solid-manifest" />
import { getRequestEvent, renderToStream } from '@solidjs/web';
import manifest from 'virtual:solid-manifest';
import { developmentAssets } from './assets';
import App from '../App';
import Document from '../Document';

const assets = developmentAssets(manifest, process.cwd());

export function render(_request: Request, context: { clientEntry: string }) {
  return renderToStream(
    () => (
      <Document developmentEntry={context.clientEntry}>
        <App />
      </Document>
    ),
    { manifest: assets, nonce: getRequestEvent()?.nonce ?? undefined },
  );
}

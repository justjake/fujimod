import type { CardEntry } from 'fujimod/emulator';
import type { JSX } from '@solidjs/web';
import { Show, createSignal } from 'solid-js';
import type { SessionState } from './session/state';
import type { SessionController } from './session/controller';
import type { DevicePanel } from './ui/layout';
import { ViewerOverlays } from './viewer/Overlays';
import { LoadingAssets } from './LoadingAssets';
import { Controls } from './ui/controls';
import { LayoutEditor } from './ui/layout-editor';
import { LayoutInspector } from './ui/LayoutInspector';
import { CardBrowser } from './ui/card';
import { Connections } from './ui/connections';
import './style.css';
import './loading.css';

export function EmulatorWorkspace(props: {
  state: SessionState;
  runtime?: SessionController;
  bodyPreview: string;
  labelledBy: string;
  workspace(element: HTMLElement): void;
  viewer(element: HTMLDivElement): void;
  lcd(element: HTMLCanvasElement): void;
  selectFile(file?: File): void;
  canDownloadCardFile?: (entry: CardEntry) => boolean;
  toolbar: JSX.Element;
  children: JSX.Element;
  onLayoutChange?: (panel: DevicePanel) => void | Promise<void>;
}) {
  const [dragging, setDragging] = createSignal(false);
  const editor = new LayoutEditor(props);
  return (
    <section
      ref={(element) => props.workspace(element)}
      class={['stage', { idle: props.state.idle, 'editing-controls': editor.editing() }]}
      id="workspace"
      aria-labelledby={props.labelledBy}
    >
      <div
        ref={(element) => props.viewer(element)}
        class={['viewer lcd-mode', { dragging: dragging() }]}
        id="viewer"
        aria-label="Camera viewer"
        style={{ '--body-preview': props.bodyPreview }}
        onDragOver={(event) => {
          event.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(event) => {
          event.preventDefault();
          setDragging(false);
          props.selectFile(event.dataTransfer?.files[0]);
        }}
      >
        <canvas
          ref={(element) => props.lcd(element)}
          id="lcd-source"
          class="flat"
          aria-label="Camera LCD"
          width="720"
          height="480"
        />
        <ViewerOverlays viewer={props.runtime?.viewer} />
        <div class="ghost" aria-hidden="true" />
        {props.toolbar}
        {props.children}
        <div id="camera-loading" class="camera-loading" hidden={!props.state.loading}>
          {/* A load without listed steps (restart, format) still uses the same row. */}
          <LoadingAssets
            assets={
              props.state.loadingAssets.length
                ? props.state.loadingAssets
                : [{ url: 'status', label: props.state.message, loaded: 0, status: 'loading' }]
            }
          />
        </div>
        <span id="status" class={{ error: props.state.error }} role="status" aria-live="polite">
          {props.state.message}
        </span>
        <span id="live-label">{props.state.live}</span>
      </div>
      <aside
        class="panel"
        id="panel"
        aria-label="Camera controls"
        hidden={!props.state.panelVisible}
      >
        <p class="fullscreen-hint">Turn your phone sideways for controls beside the screen.</p>
        <div id="controls">
          <Controls session={props.runtime?.panel} editor={editor} />
        </div>
        <CardBrowser
          engine={props.runtime?.cardEngine()}
          canDownload={props.canDownloadCardFile}
          present={(value) => props.runtime?.insertCard(value) ?? Promise.resolve()}
          format={() => props.runtime?.formatCard() ?? Promise.resolve()}
        />
        <Connections rows={props.state.connections} />
        <LayoutInspector session={props.runtime?.panel} editor={editor} />
      </aside>
    </section>
  );
}

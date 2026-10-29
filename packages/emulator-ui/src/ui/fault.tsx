import type { EngineFault } from '../engine/fault';
import { Icon } from './icons';
import { faultHeading, faultMessage, faultDiagnostics, type FaultContext } from './fault-text';
export function FaultPanel(props: {
  fault?: EngineFault;
  context: FaultContext;
  canUndo: boolean;
  undoBusy: boolean;
  canRestart: boolean;
  undo: () => void;
  restart: () => void;
  report: () => void;
}) {
  return (
    <section class="err" id="engine-status" role="alert" hidden={!props.fault}>
      <h4 id="fault-heading">
        <Icon name="alert" /> {faultHeading}
      </h4>
      <p id="fault-message">{props.fault ? faultMessage(props.fault) : ''}</p>
      <code id="fault-raw">{props.fault ? faultDiagnostics(props.fault, props.context) : ''}</code>
      <div class="act">
        <button
          class="pill primary"
          id="undo"
          hidden={!props.canUndo}
          disabled={!props.canUndo || props.undoBusy}
          onClick={() => props.undo()}
        >
          Undo 10 s
        </button>
        <button
          class="pill"
          id="restart"
          disabled={!props.canRestart}
          onClick={() => props.restart()}
        >
          Restart
        </button>
        <button class="pill" id="error-report" onClick={() => props.report()}>
          <Icon name="download" size={14} /> Download and report
        </button>
      </div>
    </section>
  );
}

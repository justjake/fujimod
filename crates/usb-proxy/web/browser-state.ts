import type { ShareHost } from './peer.ts';
import type { Session } from '../../../bindings/javascript/src/index.ts';
import type { installWebUsb } from '../../../bindings/javascript/tests/webusb-fixture.ts';
export type Fixture = ReturnType<typeof installWebUsb>;
export interface PeerTestState {
  host: ShareHost;
  logs: string[];
  fixture: Fixture;
  link: string;
  session: Session;
}
declare global {
  var peerTest: PeerTestState;
  var workerPeerTest: Omit<PeerTestState, 'link'>;
  var proxyFixture: Fixture;
}

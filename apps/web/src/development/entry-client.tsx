import { hydrate } from '@solidjs/web';
import App from '../App';
import Document from '../Document';

hydrate(
  () => (
    <Document>
      <App />
    </Document>
  ),
  document,
);

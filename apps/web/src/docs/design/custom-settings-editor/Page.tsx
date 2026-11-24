import { DesignFrame } from '../../../design/looks';
import { States } from '../../../design/screen';
import { SiteFrame } from '../../../ui/SiteFrame';
import { CustomSettingsApp } from './app';
import { Editor as SettingsEditor, SETTINGS_STATES } from './custom-settings';
import { FILM_STATES, FilmScreen } from './film-editor';

/** /docs/design/custom-settings-editor/: the custom settings editor and the
 * film editor over it, live, then each screen state for review. */
export default function CustomSettingsEditorPage() {
  return (
    <SiteFrame class="design-page">
      <DesignFrame title="Custom settings editor">
        <CustomSettingsApp />
        <States title="Custom settings editor" states={SETTINGS_STATES} render={(state) => <SettingsEditor state={state} />} />
        <States title="Film editor" states={FILM_STATES} render={(start) => <FilmScreen start={start} />} />
      </DesignFrame>
    </SiteFrame>
  );
}

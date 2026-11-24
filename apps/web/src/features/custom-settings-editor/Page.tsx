import { DesignFrame } from '../../design/looks';
import { SiteFrame } from '../../ui/SiteFrame';
import { FilmEditorCompare } from './FilmEditorCompare';
import '../../home/site.css';

export default function CustomSettingsEditorFeature() {
  return (
    <SiteFrame>
      <main class="feature-page">
        <h1 class="feature-page-title">Custom Settings Editor</h1>
        <DesignFrame title="Prototype and camera">
          <FilmEditorCompare />
        </DesignFrame>
      </main>
    </SiteFrame>
  );
}

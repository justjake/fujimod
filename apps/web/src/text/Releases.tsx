import { onSettled } from 'solid-js';
import { ReleaseNotes } from '../cameras/ReleaseNotes';
import { loadLiveReleases, releaseData, releaseDate } from '../release-data';
import TextPage, { type TextSection } from './TextPage';
import './releases.css';

function sections(): readonly TextSection[] {
  const data = releaseData();
  const releases = data.current ? [data.current, ...data.past] : data.past;
  return releases.map((release) => ({
    id: `release-${release.version}`,
    title: `Fujimod ${release.version}`,
    level: 2,
    content: () => (
      <>
        <time datetime={release.publishedAt}>
          {releaseDate.format(new Date(release.publishedAt))}
        </time>
        <ReleaseNotes title={`Fujimod ${release.version}`} text={release.notes} />
      </>
    ),
  }));
}

export default function Releases() {
  onSettled(() => void loadLiveReleases());
  return <TextPage title="Releases" class="releases-page" sections={sections()} />;
}

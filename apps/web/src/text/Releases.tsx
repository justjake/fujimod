import { ReleaseNotes } from '../cameras/ReleaseNotes';
import { releaseData } from '../release-data';
import TextPage, { type TextSection } from './TextPage';
import './releases.css';

const date = new Intl.DateTimeFormat('en-US', { dateStyle: 'long', timeZone: 'UTC' });
const releases = releaseData.current
  ? [releaseData.current, ...releaseData.past]
  : releaseData.past;
const sections: readonly TextSection[] = releases.map((release) => ({
  id: `release-${release.version}`,
  title: `Fujimod ${release.version}`,
  level: 2,
  content: () => (
    <>
      <time datetime={release.publishedAt}>{date.format(new Date(release.publishedAt))}</time>
      <ReleaseNotes title={`Fujimod ${release.version}`} text={release.notes} />
    </>
  ),
}));

export default function Releases() {
  return <TextPage title="Releases" class="releases-page" sections={sections} />;
}

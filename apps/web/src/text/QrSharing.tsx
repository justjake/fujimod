import TextPage from './TextPage';

export default function QrSharing() {
  return (
    <TextPage title="QR Sharing">
      <p>Share recipes and scan recipe and settings QR codes with FujiMod on the Fujifilm X70.</p>
      <h2>Share a recipe</h2>
      <ol>
        <li>Open a saved recipe in EDIT/SAVE CUSTOM SETTING.</li>
        <li>Choose SHOW QR CODE and point the receiving camera at the screen.</li>
      </ol>
      <h2>Scan a QR code</h2>
      <ol>
        <li>Press DRIVE and choose QR CODE SCANNER.</li>
        <li>
          Frame the entire code. Choose FOCUS LOCK or FULL PRESS in DETECT WHEN if you want to start
          a scan with the shutter button.
        </li>
        <li>
          Review the recipe or settings on the camera. Choose ADD TO LIBRARY to save a recipe.
          Choose APPLY ONCE to apply settings, or ALWAYS APPLY WITHOUT ASKING to remember that exact
          code.
        </li>
      </ol>
    </TextPage>
  );
}

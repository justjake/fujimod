export function PersonalNote() {
  return (
    <aside class="personal-note" aria-label="A note from Jake">
      <p>Hey,</p>
      <p>
        I’m making FujiMod to unlock your camera’s full potential. We’re starting with the X70, with
        23 new features and more freedom to make the camera work your way. I’d love to bring it to
        more Fujifilm cameras over time.
      </p>
      <p>
        You can try the first release by dual-booting from an SD card without flashing your camera.
        I haven’t tested every feature on camera yet (there are a lot!) so I’m sharing it for other
        X70 owners to test and help improve.
      </p>
      <p>
        Even without an X70, you can try FujiMod in your browser. The emulator is a bit slow, but
        you can enable webcam input and give it a go.
      </p>
      <p>
        Due to copyright, we can’t distribute ready-made builds of FujiMod since it’s based on the
        official firmware. Use the search box near the release notes to find the download link from
        Fujifilm for your firmware. Then upload it to this page so we can build FujiMod for your
        camera. <a href="#software">Get started below</a>
      </p>
      <p class="signature">
        Yours truly,
        <br />
        <span class="signature-name">Jake</span>
      </p>
    </aside>
  );
}

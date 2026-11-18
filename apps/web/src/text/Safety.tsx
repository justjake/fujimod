import TextPage, { type TextSection } from './TextPage';

const sections: readonly TextSection[] = [
  {
    id: 'risk',
    title: 'Risk',
    level: 2,
    content: () => (
      <>
        <p>
          In general, running custom firmware on your camera is extremely risky, much more so than
          running third-party software on a device like a laptop or phone, where the operating
          system enforces rules that protect your hardware and limit access from unprivileged
          software. In Fujifilm camera firmware, there is no privilege boundary. Any code in the
          firmware can arbitrarily control the camera hardware, write to camera internal storage or
          the SD card, potentially crash your lens or burn out autofocus motors, etc. The two most
          direct risks are:
        </p>
        <ul>
          <li>
            Updating the camera’s internal firmware to a new version that crashes during boot due to
            a bug, rendering the camera unable to function, aka “bricking” the camera. There are
            numerous reports on Reddit of users bricking their cameras by overwriting their camera’s
            internal firmware through the manufacturer’s firmware update flow.
          </li>
          <li>
            Inadvertently damaging or corrupting the camera’s internal factory calibration data.
            Unlike camera firmware, which is the same for every copy of a camera body, calibration
            data is unique and specific to a single device. While original firmware can be
            downloaded from the internet, calibration data can only be restored from a backup or
            recalibrated at an authorized service center. We are not aware of any reports of
            corrupted calibration data, but we believe this to be the highest risk to your camera
            from custom firmware.
          </li>
        </ul>
        <p>
          Fujimod specifically is experimental software developed with heavy use of AI assistance,
          aka “vibe coding”. We do not expect our contributors to read the code they contribute. We
          offer the software to you without any warranty, expressed or implied, under a{' '}
          <a href="https://github.com/justjake/fujimod/blob/main/LICENSE">
            license that disclaims us of all liability
          </a>
          .
        </p>
      </>
    ),
  },
  {
    id: 'mitigations',
    title: 'Mitigations',
    level: 2,
    content: () => (
      <>
        <p>We use various engineering techniques to attempt to mitigate these risks.</p>
      </>
    ),
  },
  {
    id: 'emulator-testing',
    title: 'Emulator testing',
    level: 3,
    content: () => (
      <>
        <p>
          The Fujimod firmware is co-developed with{' '}
          <a href="https://github.com/justjake/fujimod">an open source emulator framework</a> that
          runs actual camera firmware talking to simulated hardware. This allows us to test firmware
          changes on a computer in an automated way by feeding controlled inputs like button
          presses, Wi-Fi signals, or SD card contents to the emulator and checking that the firmware
          does what we expect. This is our most powerful and universal tool for catching and
          preventing bugs, but our theoretical model of the camera hardware is not perfect. We may
          model a hardware device incorrectly by making the wrong assumptions or deductions when
          studying the camera, or miss some kind of unknown-unknown entirely. Therefore, passing
          emulator tests can never be proof that the firmware is bug-free.
        </p>
      </>
    ),
  },
  {
    id: 'avoid-use-of-internal-storage',
    title: 'Avoid use of internal storage',
    level: 3,
    content: () => (
      <>
        <p>
          The internal storage on your camera is the domain of the camera’s manufacturer. While
          there is some spare space available that we could use to integrate Fujimod features and
          store them in internal storage, we view the risk of incorrectly using internal storage as
          too high for it to be worth it. And on newer cameras, Fujifilm may update the camera
          firmware in the future to use that space, and we do not want to produce any conflicts or
          forward corruption. Instead, Fujimod stores all of its settings in JSON files on the SD
          card.
        </p>
      </>
    ),
  },
  {
    id: 'sd-card-dual-boot',
    title: 'SD card dual boot',
    level: 3,
    content: () => (
      <>
        <p>
          Almost all Fujifilm cameras support loading the entire camera firmware from an SD card
          instead of loading it from the internal storage. This feature is provided by the boot ROM,
          an immutable program frozen onto the camera’s mainboard during manufacturing. Based on our
          research, this feature is used in Fujifilm service centers to initialize new camera
          mainboards and install the firmware for the first time so that the camera can be
          calibrated.
        </p>
        <p>
          If the internal firmware on your device is ever damaged in a way that bricks the camera,
          we should be able to use SD card dual boot to boot custom software that allows restoring
          the camera into working order. However, we’ve discovered that not all bodies actually
          support SD card dual boot. For example, the X-T1’s hardware wiring does not allow for dual
          boot, at least not without physical hardware changes to the mainboard.
        </p>
        <p>
          Fujimod mitigates the risk of custom firmware rendering your camera unable to boot by only
          distributing firmware in SD card dual boot form, rather than as a firmware update file
          like FPUPDATE.DAT. By booting Fujimod on your camera from an SD card, you prove that it is
          theoretically possible to rescue your camera from a bricked state due to damaged internal
          firmware. We also build dual boot recovery images from minimally modified official
          firmware that allow flashing any firmware file onto a camera.
        </p>
        <p>
          This also means that unless you use the SD autostart function described below, the
          uninstall process for Fujimod is a single step: turn off the camera by ejecting the
          battery.
        </p>
        <p>
          Unfortunately, manually booting your camera from SD card every time you start it is
          annoying, as it requires holding down sometimes cumbersome button combinations when
          switching the camera on.
        </p>
      </>
    ),
  },
  {
    id: 'sd-boot-autostart',
    title: 'SD boot autostart',
    level: 3,
    content: () => (
      <>
        <p>
          We view wholesale replacement of internal firmware through the typical “firmware flashing”
          update process as both irresponsibly risky and unnecessarily inconvenient. It is possible
          to brick a Fujifilm camera even when installing known good official firmware update files
          if power is lost during the update process. Updates are lengthy, and we plan to release
          new versions of Fujimod regularly. I think it would be annoying to flash custom firmware
          onto a camera even a handful of times, and each time you do, you incur the risk of a
          brick.
        </p>
        <p>
          Instead, our goal is to find the minimum modification to your camera’s internal storage
          necessary to automatically boot from the SD card, only when the SD card contains a special
          processor-specific bootloader file. If the bootloader file is not present on the SD card,
          the camera boots from its mostly-original internal firmware as usual.
        </p>
        <p>
          This modification can typically be made very small and does most of its work by calling
          functions already present on the camera’s boot ROM. After it is installed once, we can
          continue to update the custom firmware by writing new files to the SD card. This also
          means that you could potentially replace Fujimod with some other custom firmware
          modification by replacing the Fujimod bootloader file with one of your own.
        </p>
      </>
    ),
  },
  {
    id: 'atomic-updates',
    title: 'Atomic updates',
    level: 3,
    content: () => (
      <>
        <p>
          We want updating Fujimod to be more reliable than updating to new versions of official
          firmware. When we install an update of Fujimod onto the SD card, we keep around the prior
          version of Fujimod, and we update a settings file on the SD card to tell the bootloader to
          attempt to boot the new version. Early in the boot process, the bootloader records an
          attempt to boot to the settings file, and uses the camera’s interval shooting function to
          schedule an automatic power reset a few seconds in the future. If the boot does not
          complete successfully within that time limit, the camera resets and tries to boot again.
          When the bootloader sees multiple attempts to boot a version of Fujimod with no successes,
          it considers the update failed and either goes back to booting the previous version of
          Fujimod or, if none is available, boots the standard firmware from internal storage.
        </p>
        <p>
          This feature is called atomic updates because the update either succeeds and you get to
          use the new version, or it fails and you’re returned to using the previous version, never
          stranding you in between.
        </p>
        <p>
          The failsafe that boots your camera’s internal firmware also means that if any changes
          happen to your SD card that cause a working version of Fujimod to stop working, for
          example due to a bug in the handling of a specific setting, you are likely to still have a
          working camera, even if you’re away from a computer on an overnight trip with only a
          single SD card.
        </p>
      </>
    ),
  },
];

export default function Safety() {
  return (
    <TextPage title="Safety" sections={sections}>
      <p class="lede">
        This page describes the risks of custom firmware and how we attempt to mitigate those risks,
        so you can make an informed decision about using Fujimod.
      </p>
    </TextPage>
  );
}

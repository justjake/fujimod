//! Autostart: the card's own installer writes the boot ROM's first
//! stage into internal flash, and later opens start from it.
//!
//! The card's FUJIMOD SET-UP page exposes BOOT SETUP's CHANGE, whose
//! AUTOMATIC choice backs the internal data up to the card and then has
//! `sd-boot-v2-stage1-internal-flash-install` rewrite the stage-1 blocks the
//! boot ROM loads. The builder drives that page with the same presses a person
//! makes, keeps the blocks the installer wrote, and every later open programs
//! them back into a stock boot with nothing held.
//!
//! Finding the page needs no firmware addresses: the mod attaches its page to
//! the stock SET-UP page, publishes row captions the stock text table does not
//! hold, and gives BOOT SETUP's CHANGE page exactly two leaf rows, MANUAL and
//! AUTOMATIC. The search reads descriptors only; presses start once the page
//! is known.
use fujimod_catalog::nand_boot::FirmwareNandBoot;
use fujimod_catalog_remote::{Facts, FirmwareObservation, MenuTables};
use fujimod_emulator_camera::{OpenError, Opened, Stage, read_flash};
use fujimod_emulator_session::Session;
use fujimod_remote::{
    BoardRemote, Control, Remote, RemoteReader,
    board::Options as RemoteOptions,
    protocol::{Input, InputState},
};
use serde_json::{Value, json};

/// The card settings file whose menu style the installer run changes.
const SETTINGS_FILE: &str = "FUJIMOD/SETTINGS.JSN";

/// The remote role the menu facts give the SET-UP page.
const SETUP_ROLE: &str = "setup";

/// Rows one menu descriptor may hold before it is read as invalid.
const ROW_LIMIT: usize = 1024;

/// Guest seconds the installer may take; AUTOMATIC backs the internal data up
/// before it writes the boot blocks.
const INSTALL_SECONDS: u64 = 180;

/// Guest milliseconds a press settles before the next observation.
const SETTLE_MILLIS: u64 = 250;

/// Guest milliseconds the welcome sheet and the menu take to appear.
const MENU_MILLIS: u64 = 3_000;

/// Presses the welcome sheet and the menu may take together.
const PRESS_LIMIT: usize = 4;

/// The card's files with stock-style menus, which expose the mod's pages to
/// the stock menu descriptors the builder reads.
///
/// Only the installer run sets this: a later boot loads the card as the caller
/// gave it, so the camera a caller receives keeps the style it asked for.
pub fn stock_menus(files: &[(String, Vec<u8>)]) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
    let mut files = files.to_vec();
    let existing = files
        .iter()
        .position(|(name, _)| name.eq_ignore_ascii_case(SETTINGS_FILE));
    let mut value: Value = match existing {
        Some(index) => serde_json::from_slice(&files[index].1).map_err(|error| {
            OpenError::at(Stage::BootRom, format!("card {SETTINGS_FILE}: {error}"))
        })?,
        None => json!({}),
    };
    let menu = value
        .as_object_mut()
        .and_then(|settings| {
            settings
                .entry("menu")
                .or_insert_with(|| json!({}))
                .as_object_mut()
        })
        .ok_or_else(|| {
            OpenError::at(
                Stage::BootRom,
                format!("card {SETTINGS_FILE}: menu is not an object"),
            )
        })?;
    menu.insert("style".into(), json!("stock"));
    let bytes = serde_json::to_vec(&value)
        .map_err(|error| OpenError::at(Stage::BootRom, format!("card {SETTINGS_FILE}: {error}")))?;
    match existing {
        Some(index) => files[index].1 = bytes,
        None => files.push((SETTINGS_FILE.to_owned(), bytes)),
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Drive the card's installer to AUTOMATIC and return the stage-1 blocks it
/// wrote, after powering the camera off.
pub(crate) fn install(opened: &mut Opened, nand: &FirmwareNandBoot) -> Result<Vec<u8>, OpenError> {
    let facts = fujimod_emulator_camera::remote_facts(&opened.resolved)
        .map_err(|error| OpenError::at(Stage::Rec, error))?;
    let menus = Menus::new(&facts);
    let driver = Driver {
        facts: &facts,
        ticks: opened.camera.ticks_per_second,
        quantum: opened.camera.quantum,
    };
    let stock = read_flash(opened.session(), nand)?;
    driver.open_menus(opened.session_mut(), &menus)?;
    driver.open_setup(opened.session_mut(), &menus)?;
    let installer = menus.find_installer(opened.session())?;
    driver.choose_automatic(opened.session_mut(), &menus, &installer)?;
    driver.wait_installed(opened.session_mut(), nand, &stock)?;
    driver.power_off(opened.session_mut())?;
    let written = read_flash(opened.session(), nand)?;
    if written == stock {
        return Err(OpenError::at(
            Stage::Rec,
            "the installer reported no change to the stage-1 blocks",
        ));
    }
    Ok(written)
}

/// One row of a native menu descriptor.
#[derive(Clone, Copy, Debug)]
struct Row {
    key: u32,
    child: u64,
}

/// The presses from the SET-UP page to AUTOMATIC.
struct Installer {
    page: u64,
    install: usize,
    install_node: u64,
    change: usize,
    change_node: u64,
}

/// The stock menu descriptors and the text table they draw from.
struct Menus<'a> {
    tables: &'a MenuTables,
    state: &'a FirmwareObservation,
    last_caption: u32,
}

impl<'a> Menus<'a> {
    fn new(facts: &'a Facts) -> Self {
        Self {
            tables: &facts.firmware.menus,
            state: &facts.firmware.state,
            last_caption: facts.firmware.resources.last_caption,
        }
    }

    fn active(&self, session: &Session) -> Result<bool, OpenError> {
        Ok(self.word(session, self.state.menu_active)? != 0)
    }

    fn node(&self, session: &Session) -> Result<u64, OpenError> {
        Ok(u64::from(self.word(session, self.state.menu_node)?))
    }

    fn word(&self, session: &Session, address: u64) -> Result<u32, OpenError> {
        let mut bytes = [0u8; 4];
        session
            .board
            .read_physical(address, &mut bytes)
            .map_err(|error| {
                OpenError::at(Stage::Rec, format!("menu word at {address:#x}: {error}"))
            })?;
        Ok(u32::from_le_bytes(bytes))
    }

    /// The menu id a descriptor names.
    fn menu_id(&self, session: &Session, pointer: u64) -> Result<u32, OpenError> {
        let at = pointer + u64::from(self.tables.layout.fields.menu_id);
        self.word(session, at)
    }

    /// The focused row of the open menu.
    fn cursor(&self, session: &Session) -> Result<u32, OpenError> {
        let at = self.tables.cursor_address + u64::from(self.tables.cursor.fields.cursor);
        self.word(session, at)
    }

    /// Whether a node pointer holds a descriptor this menu layout describes.
    ///
    /// A mod's pages take menu ids above the stock root table, so only the
    /// kind and page size are checked.
    fn descriptor(&self, session: &Session, pointer: u64) -> Result<bool, OpenError> {
        let layout = &self.tables.layout;
        let at = |field: u32| pointer + u64::from(field);
        Ok(
            self.word(session, at(layout.fields.type_code))? <= layout.max_kind
                && self.word(session, at(layout.fields.rows_per_page))? <= ROW_LIMIT as u32,
        )
    }

    /// A descriptor's rows, in the order the firmware lists them.
    fn rows(&self, session: &Session, pointer: u64) -> Result<Vec<Row>, OpenError> {
        let layout = &self.tables.layout;
        let array = u64::from(self.word(session, pointer + u64::from(layout.fields.item_array))?);
        if array == 0 {
            return Ok(Vec::new());
        }
        let mut rows = Vec::new();
        for index in 0..=ROW_LIMIT {
            let at = array + u64::from(index as u32 * layout.row_size);
            let key = self.word(session, at + u64::from(layout.row_key))?;
            let child = u64::from(self.word(session, at + u64::from(layout.row_child))?);
            if key == layout.sentinel && child == 0 {
                return Ok(rows);
            }
            if index == ROW_LIMIT {
                break;
            }
            rows.push(Row { key, child });
        }
        Err(OpenError::at(
            Stage::Rec,
            format!("menu descriptor at {pointer:#x} has no row terminator"),
        ))
    }

    /// Whether a row's caption comes from the running mod rather than the
    /// stock text table. A mod's page draws its rows from records it writes
    /// while the camera runs, so their captions are pointers above every stock
    /// caption resource.
    fn published(&self, session: &Session, node: u64, index: usize) -> Result<bool, OpenError> {
        let layout = &self.tables.layout;
        let captions =
            u64::from(self.word(session, node + u64::from(layout.fields.handlers_pointer))?);
        if captions == 0 {
            return Ok(false);
        }
        let stride = u64::from(layout.caption_words) * 4;
        for word in 0..layout.caption_words {
            let at = captions + index as u64 * stride + u64::from(word) * 4;
            let resource = self.word(session, at)?;
            if resource != 0 && resource != layout.sentinel && resource > self.last_caption {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A row that opens a page of exactly two leaf rows, the shape BOOT
    /// SETUP's CHANGE page has, with that page's node.
    fn two_choices(
        &self,
        session: &Session,
        pointer: u64,
    ) -> Result<Option<(usize, u64)>, OpenError> {
        for (index, row) in self.rows(session, pointer)?.into_iter().enumerate() {
            if row.child == 0 || !self.descriptor(session, row.child)? {
                continue;
            }
            let choices = self.rows(session, row.child)?;
            if choices.len() != 2 {
                continue;
            }
            // A choice's own descriptor, when it has one, holds no rows.
            let mut leaf = true;
            for choice in &choices {
                if choice.child == 0 {
                    continue;
                }
                leaf &= self.descriptor(session, choice.child)?
                    && self.rows(session, choice.child)?.is_empty();
            }
            if leaf {
                return Ok(Some((index, row.child)));
            }
        }
        Ok(None)
    }

    /// Find FUJIMOD SET-UP, its CHANGE row and the page MANUAL and AUTOMATIC
    /// share, from the SET-UP page the caller opened.
    ///
    /// FUJIMOD SET-UP is the mod's row on SET-UP whose page holds a two-row
    /// page: BOOT SETUP's CHANGE. The mod's rows are the ones whose captions
    /// the stock text table does not hold.
    fn find_installer(&self, session: &Session) -> Result<Installer, OpenError> {
        let page = self.node(session)?;
        for (index, row) in self.rows(session, page)?.into_iter().enumerate() {
            if row.child == 0 || !self.descriptor(session, row.child)? {
                continue;
            }
            if !self.published(session, page, index)? {
                continue;
            }
            if let Some((change, change_node)) = self.two_choices(session, row.child)? {
                return Ok(Installer {
                    page,
                    install: index,
                    install_node: row.child,
                    change,
                    change_node,
                });
            }
        }
        Err(OpenError::at(
            Stage::Rec,
            format!("the card's SET-UP page at {page:#x} holds no BOOT SETUP page"),
        ))
    }
}

/// Presses and guest time for one installation.
struct Driver<'a> {
    facts: &'a Facts,
    ticks: u64,
    quantum: usize,
}

impl Driver<'_> {
    fn options(&self, session: &Session) -> Result<RemoteOptions, OpenError> {
        fujimod_emulator_camera::remote_options(session, self.quantum)
            .map_err(|error| OpenError::at(Stage::Rec, error))
    }

    /// One press, held a quarter second and left to settle.
    fn press(&self, session: &mut Session, control: &str) -> Result<(), OpenError> {
        let options = self.options(session)?;
        let mut remote = BoardRemote::new(&mut *session, self.facts.clone(), options)
            .map_err(|error| OpenError::at(Stage::Rec, format!("controls: {error}")))?;
        let deadline = remote.now() + 10 * self.ticks;
        let control = Control::new(control);
        futures_lite::future::block_on(remote.set_input(
            &Input {
                control: control.clone(),
                state: InputState::Pressed,
            },
            deadline,
        ))
        .map_err(|error| OpenError::at(Stage::Rec, format!("{}: {error}", control.id())))?;
        let held = remote.now() + self.ticks / 4;
        futures_lite::future::block_on(remote.advance_until(held))
            .map_err(|error| OpenError::at(Stage::Rec, error))?;
        futures_lite::future::block_on(remote.set_input(
            &Input {
                control: control.clone(),
                state: InputState::Released,
            },
            deadline,
        ))
        .map_err(|error| OpenError::at(Stage::Rec, format!("{}: {error}", control.id())))?;
        let _ = remote.into_session();
        self.advance(session, SETTLE_MILLIS)
    }

    /// Run the guest for a span of virtual time.
    fn advance(&self, session: &mut Session, millis: u64) -> Result<(), OpenError> {
        let options = self.options(session)?;
        let mut remote = BoardRemote::new(&mut *session, self.facts.clone(), options)
            .map_err(|error| OpenError::at(Stage::Rec, format!("controls: {error}")))?;
        let until = remote.now() + self.ticks * millis / 1000;
        futures_lite::future::block_on(remote.advance_until(until))
            .map_err(|error| OpenError::at(Stage::Rec, error))?;
        let _ = remote.into_session();
        Ok(())
    }

    /// Dismiss the welcome sheet if it is up and open the menus. The first
    /// press after REC sometimes goes to the sheet the camera was showing, so
    /// every press is followed by a look.
    fn open_menus(&self, session: &mut Session, menus: &Menus<'_>) -> Result<(), OpenError> {
        for _ in 0..PRESS_LIMIT {
            if menus.active(session)? {
                return Ok(());
            }
            self.press(session, "menu_ok_button")?;
            self.advance(session, MENU_MILLIS)?;
        }
        if menus.active(session)? {
            Ok(())
        } else {
            Err(OpenError::at(
                Stage::Rec,
                "the camera did not open its menu",
            ))
        }
    }

    /// The menu id the facts give the SET-UP page, and the rail key of its
    /// first tab.
    fn setup_route(&self) -> Result<(u32, u32), OpenError> {
        let menus = &self.facts.firmware.menus;
        let code = menus.roles.get(SETUP_ROLE).copied().ok_or_else(|| {
            OpenError::at(
                Stage::Rec,
                format!(
                    "the firmware's menu facts name no {SETUP_ROLE} menu: {:?}",
                    menus.roles.keys().collect::<Vec<_>>()
                ),
            )
        })?;
        let tab = menus
            .root_routes
            .get(&code)
            .and_then(|route| route.tab)
            .or_else(|| menus.page_tabs.get(&code).copied())
            .ok_or_else(|| {
                OpenError::at(
                    Stage::Rec,
                    format!("the firmware's menu facts give {SETUP_ROLE} no tab"),
                )
            })?;
        Ok((code, tab))
    }

    /// Walk the menu rail to the SET-UP tab and open its page.
    fn open_setup(&self, session: &mut Session, menus: &Menus<'_>) -> Result<(), OpenError> {
        let (code, tab) = self.setup_route()?;
        if menus.menu_id(session, menus.node(session)?)? == code {
            return Ok(());
        }
        self.press(session, "selector.left")?;
        for _ in 0..64 {
            let rows = menus.rows(session, menus.node(session)?)?;
            let index = rows.iter().position(|row| row.key == tab).ok_or_else(|| {
                OpenError::at(
                    Stage::Rec,
                    format!("the menu rail holds no {SETUP_ROLE} tab {tab}"),
                )
            })?;
            let cursor = menus.cursor(session)? as usize;
            if cursor == index {
                break;
            }
            self.press(
                session,
                if cursor < index {
                    "selector.down"
                } else {
                    "selector.up"
                },
            )?;
        }
        self.press(session, "selector.right")?;
        let node = menus.node(session)?;
        let opened = menus.menu_id(session, node)?;
        if opened != code {
            return Err(OpenError::at(
                Stage::Rec,
                format!("the {SETUP_ROLE} tab opened menu {opened}, not {code}"),
            ));
        }
        Ok(())
    }

    /// Focus a row of the open node.
    fn focus(
        &self,
        session: &mut Session,
        menus: &Menus<'_>,
        index: usize,
    ) -> Result<(), OpenError> {
        for _ in 0..64 {
            let cursor = menus.cursor(session)? as usize;
            if cursor == index {
                return Ok(());
            }
            self.press(
                session,
                if cursor < index {
                    "selector.down"
                } else {
                    "selector.up"
                },
            )?;
        }
        Err(OpenError::at(
            Stage::Rec,
            format!("the menu would not focus row {index}"),
        ))
    }

    /// Walk from the SET-UP page to AUTOMATIC and choose it.
    fn choose_automatic(
        &self,
        session: &mut Session,
        menus: &Menus<'_>,
        installer: &Installer,
    ) -> Result<(), OpenError> {
        let expected = [
            (installer.page, installer.install, installer.install_node),
            (
                installer.install_node,
                installer.change,
                installer.change_node,
            ),
        ];
        for (node, index, next) in expected {
            let current = menus.node(session)?;
            if current != node {
                return Err(OpenError::at(
                    Stage::Rec,
                    format!("the installer path expected menu node {node:#x}, found {current:#x}"),
                ));
            }
            self.focus(session, menus, index)?;
            self.press(session, "menu_ok_button")?;
            let current = menus.node(session)?;
            if current != next {
                return Err(OpenError::at(
                    Stage::Rec,
                    format!("the installer path expected menu node {next:#x}, found {current:#x}"),
                ));
            }
        }
        // MANUAL is the first choice and AUTOMATIC the second; AUTOMATIC
        // backs the internal data up before it writes the boot blocks.
        self.focus(session, menus, 1)?;
        self.press(session, "menu_ok_button")
    }

    /// Wait for the installer to rewrite the stage-1 blocks.
    fn wait_installed(
        &self,
        session: &mut Session,
        nand: &FirmwareNandBoot,
        stock: &[u8],
    ) -> Result<(), OpenError> {
        let deadline = session.board.now() + self.ticks * INSTALL_SECONDS;
        loop {
            self.advance(session, 1_000)?;
            if read_flash(session, nand)? != stock {
                return Ok(());
            }
            if session.board.now() >= deadline {
                return Err(OpenError::at(
                    Stage::Rec,
                    format!(
                        "the installer did not write the boot blocks within {INSTALL_SECONDS} seconds"
                    ),
                ));
            }
        }
    }

    /// Switch the camera off, as the installer's screen asks.
    fn power_off(&self, session: &mut Session) -> Result<(), OpenError> {
        session
            .set_position("power_switch", "OFF")
            .map_err(|error| OpenError::at(Stage::Rec, format!("power switch: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_run_keeps_the_callers_settings_and_asks_for_stock_menus() {
        let files = vec![
            (
                "FUJIMOD/SETTINGS.JSN".to_owned(),
                br#"{"menu":{"style":"fujimod","quick_start":10},"af":"kept"}"#.to_vec(),
            ),
            ("FFBOOT.BIN".to_owned(), vec![1, 2]),
        ];
        let styled = stock_menus(&files).unwrap();
        assert_eq!(
            styled
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["FFBOOT.BIN", "FUJIMOD/SETTINGS.JSN"]
        );
        let settings: Value = serde_json::from_slice(&styled[1].1).unwrap();
        assert_eq!(settings["menu"]["style"], "stock");
        assert_eq!(settings["menu"]["quick_start"], 10);
        assert_eq!(settings["af"], "kept");

        let added = stock_menus(&[("FFBOOT.BIN".to_owned(), vec![1, 2])]).unwrap();
        assert_eq!(added.len(), 2);
        let settings: Value = serde_json::from_slice(&added[1].1).unwrap();
        assert_eq!(settings["menu"]["style"], "stock");
    }
}

/// The internal flash an autostart open starts from, installing it once.
pub(crate) fn flash(
    state: &fujimod_emulator_camera::StateDir,
    library: &fujimod_emulator_camera::Library,
    request: &crate::Request,
    files: &[(String, Vec<u8>)],
) -> Result<fujimod_emulator_camera::Flash, crate::Error> {
    let path = crate::flash_path(state, &request.firmware, files);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(fujimod_emulator_camera::Flash::Image(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let installed = install_flash(state, library, request, files)?;
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory).map_err(|error| {
                    crate::Error::Missing(format!("{}: {error}", directory.display()))
                })?;
            }
            std::fs::write(&path, &installed)
                .map_err(|error| crate::Error::Missing(format!("{}: {error}", path.display())))?;
            Ok(fujimod_emulator_camera::Flash::Image(installed))
        }
        Err(error) => Err(crate::Error::Missing(format!(
            "{}: {error}",
            path.display()
        ))),
    }
}

/// Make the autostart flash the way an owner does: boot the card, choose
/// AUTOMATIC, and keep the stage-1 blocks the installer wrote.
fn install_flash(
    state: &fujimod_emulator_camera::StateDir,
    library: &fujimod_emulator_camera::Library,
    request: &crate::Request,
    files: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, crate::Error> {
    let menus = stock_menus(files)?;
    let mut opened = fujimod_emulator_camera::open(
        library,
        state,
        fujimod_emulator_camera::Request {
            firmware: request.firmware.clone(),
            files: request.files.clone(),
            overlays: request.overlays.clone(),
            start: fujimod_emulator_camera::Start::Held {
                combination: "sd".into(),
            },
            flash: fujimod_emulator_camera::Flash::Stock,
            card: fujimod_emulator_camera::CardInputs {
                files: menus,
                ..Default::default()
            },
            policy: request.policy,
            engine: request.engine,
            cache: fujimod_emulator_camera::BootCache::Bypass,
            run_to_rec: true,
            nand_image: None,
            face_result_profile: None,
            kept: Default::default(),
        },
    )?;
    let nand = library.nand_boot(&request.firmware)?;
    Ok(install(&mut opened, nand)?)
}

use crate::core::Core;
use anyhow::Result;
use bigdecimal::{BigDecimal, ToPrimitive};
use cursive::Cursive;
use cursive::event::{Event, Key};
use cursive::traits::*;
use cursive::views::{
    Button, Dialog, EditView, LinearLayout, Panel, ResizedView, TextContent, TextView,
};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tracing::*;

#[derive(Clone, Copy)]
enum Unit {
    Btc,
    Sats,
}

/// Convert an amount between BTC and Satoshi units.
fn convert_amount(amount: &BigDecimal, from: Unit, to: Unit) -> BigDecimal {
    match (from, to) {
        (Unit::Btc, Unit::Sats) => amount * BigDecimal::from(100_000_000u64),
        (Unit::Sats, Unit::Btc) => amount / BigDecimal::from(100_000_000u64),
        _ => amount.clone(),
    }
}

/// Initialize and run the user interface.
pub fn run_ui(core: Arc<Core>, balance_content: TextContent) -> Result<()> {
    info!("Initializing UI");
    let mut siv = cursive::default();
    setup_siv(&mut siv, core.clone(), balance_content);
    info!("Starting UI event loop");
    siv.run();
    info!("UI event loop ended");
    Ok(())
}

/// Set up the Cursive interface with all necessary components and callbacks.
fn setup_siv(siv: &mut Cursive, core: Arc<Core>, balance_content: TextContent) {
    siv.set_autorefresh(true);
    siv.set_window_title("BTC wallet".to_string());
    siv.set_user_data(core.clone());
    siv.add_global_callback('q', |s| {
        info!("Quit command received");
        s.quit()
    });
    setup_menubar(siv);
    setup_layout(siv, balance_content);
    siv.add_global_callback(Event::Key(Key::Esc), |siv| siv.select_menubar());
    siv.select_menubar();
}

/// Show contacts management dialog with table view and pagination
fn show_contacts_dialog(s: &mut Cursive) {
    const ITEMS_PER_PAGE: usize = 10;

    let core = s
        .user_data::<Arc<Core>>()
        .expect("Core missing from user_data")
        .clone();
    let config = core.config.read().unwrap();
    let contacts: Vec<_> = config.contacts.clone();
    drop(config);

    if contacts.is_empty() {
        s.add_layer(
            Dialog::around(TextView::new("(No contacts)"))
                .title("Contacts")
                .button("Add Contact", move |siv| {
                    siv.pop_layer();
                    show_add_contact_standalone(siv);
                })
                .button("Close", |siv| {
                    siv.pop_layer();
                }),
        );
        return;
    }

    // Create table rows for current page
    let current_page = 0;
    let total_pages = (contacts.len() + ITEMS_PER_PAGE - 1) / ITEMS_PER_PAGE;

    create_contacts_table_page(s, contacts, current_page, total_pages, ITEMS_PER_PAGE);
}

/// Create a paginated table view of contacts
fn create_contacts_table_page(
    s: &mut Cursive,
    contacts: Vec<crate::core::Recipient>,
    current_page: usize,
    total_pages: usize,
    items_per_page: usize,
) {
    let start_idx = current_page * items_per_page;
    let end_idx = (start_idx + items_per_page).min(contacts.len());
    let page_contacts = &contacts[start_idx..end_idx];

    // Create table header
    let header = LinearLayout::horizontal()
        .child(ResizedView::with_fixed_width(20, TextView::new("Name")))
        .child(ResizedView::with_fixed_width(40, TextView::new("Address")))
        .child(ResizedView::with_fixed_width(20, TextView::new("Actions")));

    // Create table rows
    let mut rows = LinearLayout::vertical();

    for contact in page_contacts {
        let contact_name = contact.name.clone();
        let contact_address = contact.address.clone();

        // Format address to fit in column (truncate if too long)
        let display_address = if contact_address.len() > 35 {
            format!("{}...", &contact_address[..32])
        } else {
            contact_address.clone()
        };

        let row = LinearLayout::horizontal()
            .child(ResizedView::with_fixed_width(
                20,
                TextView::new(&contact_name),
            ))
            .child(ResizedView::with_fixed_width(
                40,
                TextView::new(&display_address),
            ))
            .child(ResizedView::with_fixed_width(
                20,
                LinearLayout::horizontal()
                    .child(Button::new("Send", {
                        let name = contact_name.clone();
                        let addr = contact_address.clone();
                        move |siv| {
                            siv.pop_layer(); // Close contacts dialog
                            show_transaction_dialog(siv, Some((name.clone(), addr.clone())));
                        }
                    }))
                    .child(Button::new("Delete", {
                        let name = contact_name.clone();
                        let addr = contact_address.clone();
                        move |siv| {
                            delete_contact(siv, name.clone(), addr.clone());
                        }
                    })),
            ));

        rows.add_child(row);
    }

    // Create pagination controls
    let mut pagination = LinearLayout::horizontal();

    if total_pages > 1 {
        let contacts_prev = contacts.clone();
        let prev_enabled = current_page > 0;

        if prev_enabled {
            pagination.add_child(Button::new("◀ Prev", {
                move |siv| {
                    siv.pop_layer();
                    create_contacts_table_page(
                        siv,
                        contacts_prev.clone(),
                        current_page - 1,
                        total_pages,
                        items_per_page,
                    );
                }
            }));
        } else {
            pagination.add_child(TextView::new("◀ Prev"));
        }

        pagination.add_child(TextView::new(format!(
            "Page {} of {}",
            current_page + 1,
            total_pages
        )));

        let contacts_next = contacts.clone();
        let next_enabled = current_page < total_pages - 1;

        if next_enabled {
            pagination.add_child(Button::new("Next ▶", {
                move |siv| {
                    siv.pop_layer();
                    create_contacts_table_page(
                        siv,
                        contacts_next.clone(),
                        current_page + 1,
                        total_pages,
                        items_per_page,
                    );
                }
            }));
        } else {
            pagination.add_child(TextView::new("Next ▶"));
        }
    }

    // Combine header, rows, and pagination
    let table_content = LinearLayout::vertical()
        .child(header)
        .child(rows)
        .child(TextView::new("")) // Spacer
        .child(pagination);

    s.add_layer(
        Dialog::around(table_content)
            .title("Contacts")
            .button("Add Contact", move |siv| {
                siv.pop_layer();
                show_add_contact_standalone(siv);
            })
            .button("Close", |siv| {
                siv.pop_layer();
            }),
    );
}

/// Delete a contact with confirmation
fn delete_contact(s: &mut Cursive, name: String, address: String) {
    s.add_layer(
        Dialog::text(format!(
            "Are you sure you want to delete contact '{}'?\nAddress: {}",
            name, address
        ))
        .title("Delete Contact")
        .button("Delete", move |s| {
            let core = s
                .user_data::<Arc<Core>>()
                .expect("Core missing from user_data")
                .clone();
            let name = name.clone();
            match core.remove_contact(name.as_str()) {
                Ok(_) => {
                    s.pop_layer(); // Close confirmation dialog
                    s.pop_layer(); // Close contacts dialog
                    show_contacts_dialog(s); // Refresh contacts dialog
                    show_success_dialog(s, format!("Contact '{}' deleted successfully", name));
                }
                Err(e) => {
                    show_error_dialog(s, format!("{}", e));
                }
            }
        })
        .button("Cancel", |siv| {
            siv.pop_layer();
        }),
    );
}

/// Show dialog to add contact (standalone, not from transaction)
fn show_add_contact_standalone(s: &mut Cursive) {
    let core = s
        .user_data::<Arc<Core>>()
        .expect("Core missing from user_data")
        .clone();
    s.add_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(TextView::new("Contact name:"))
                .child(EditView::new().with_name("contact_name"))
                .child(TextView::new("Bitcoin address:"))
                .child(EditView::new().with_name("contact_address")),
        )
        .title("Add Contact")
        .button("Save", move |siv| {
            let name = siv
                .call_on_name("contact_name", |view: &mut EditView| view.get_content())
                .unwrap();
            let address = siv
                .call_on_name("contact_address", |view: &mut EditView| view.get_content())
                .unwrap();

            if name.trim().is_empty() {
                show_error_dialog(siv, "Contact name cannot be empty");
                return;
            }

            if address.trim().is_empty() {
                show_error_dialog(siv, "Address cannot be empty");
                return;
            }

            match core.add_contact(name.trim().to_string(), address.trim().to_string()) {
                Ok(_) => {
                    siv.pop_layer();
                    show_success_dialog(siv, "Contact added successfully".to_string());
                }
                Err(e) => {
                    show_error_dialog(siv, format!("{}", e));
                }
            }
        })
        .button("Cancel", |siv| {
            siv.pop_layer();
        }),
    );
}

/// Set up the menu bar with "Send", "Contacts", and "Quit" options.
fn setup_menubar(siv: &mut Cursive) {
    siv.menubar()
        .add_leaf("Send", |s| show_transaction_dialog(s, None))
        .add_leaf("Contacts", |s| show_contacts_dialog(s))
        .add_leaf("Quit", |s| s.quit());

    siv.set_autohide_menu(false);
}

/// Set up the main layout of the application.
fn setup_layout(siv: &mut Cursive, balance_content: TextContent) {
    let instruction = TextView::new("Press Escape to select the top menu");
    let balance_panel = Panel::new(TextView::new_with_content(balance_content)).title("Balance");

    // Create wallet address panel
    let core = siv
        .user_data::<Arc<Core>>()
        .expect("Core missing from user_data")
        .clone();
    let wallet_address_content = TextContent::new(create_wallet_address_text(&core));
    let wallet_address_panel =
        Panel::new(TextView::new_with_content(wallet_address_content)).title("Wallet Address");

    let info_layout = create_info_layout(&core);
    let layout = LinearLayout::vertical()
        .child(instruction)
        .child(balance_panel)
        .child(wallet_address_panel)
        .child(info_layout);
    siv.add_layer(layout);
    //siv.add_fullscreen_layer(layout);
}

/// Create the wallet address text
fn create_wallet_address_text(core: &Arc<Core>) -> String {
    let addresses = core.get_addresses();
    if addresses.is_empty() {
        "(No wallet addresses)".to_string()
    } else if addresses.len() == 1 {
        addresses[0].clone()
    } else {
        addresses
            .iter()
            .enumerate()
            .map(|(idx, addr)| format!("Address {}: {}", idx + 1, addr))
            .collect::<Vec<String>>()
            .join("\n")
    }
}

/// Create the information layout containing keys and contacts.
fn create_info_layout(core: &Arc<Core>) -> LinearLayout {
    let mut info_layout = LinearLayout::horizontal();
    let config = core.config.read().unwrap();

    let keys_content = if config.my_keys.is_empty() {
        "(No keys configured)".to_string()
    } else {
        let addresses = core.get_addresses();
        config
            .my_keys
            .iter()
            .enumerate()
            .map(|(idx, key)| {
                let address = addresses
                    .get(idx)
                    .map(|a| a.as_str())
                    .unwrap_or("(address unavailable)");
                format!("{}\n  Address: {}", key.private.display(), address)
            })
            .collect::<Vec<String>>()
            .join("\n\n")
    };
    info_layout.add_child(ResizedView::with_full_width(
        Panel::new(TextView::new(keys_content)).title("Your keys"),
    ));

    let contacts_content = if config.contacts.is_empty() {
        "(No contacts)".to_string()
    } else {
        config
            .contacts
            .iter()
            .map(|contact| format!("{}\n  Address: {}", contact.name, contact.address))
            .collect::<Vec<String>>()
            .join("\n\n")
    };
    info_layout.add_child(ResizedView::with_full_width(
        Panel::new(TextView::new(contacts_content)).title("Contacts"),
    ));
    info_layout
}

/// Display the transaction dialog with optional pre-filled recipient.
fn show_transaction_dialog(s: &mut Cursive, recipient: Option<(String, String)>) {
    info!("Showing send transaction dialog");
    let unit = Arc::new(Mutex::new(Unit::Btc));

    // Pre-fill recipient if provided
    let initial_recipient = recipient.map(|(name, _address)| name);
    let layout = create_transaction_layout(unit.clone(), initial_recipient);

    s.add_layer(
        Dialog::around(layout)
            .title("Send Transaction")
            .button("Send", move |siv| {
                send_transaction(siv, *unit.lock().unwrap())
            })
            .button("Cancel", |siv| {
                debug!("Transaction cancelled");
                siv.pop_layer();
            }),
    );
}

/// Create the layout for the transaction dialog.
fn create_transaction_layout(
    unit: Arc<Mutex<Unit>>,
    initial_recipient: Option<String>,
) -> LinearLayout {
    let mut recipient_view = EditView::new();
    if let Some(recipient) = initial_recipient {
        recipient_view.set_content(recipient);
    }
    LinearLayout::vertical()
        .child(TextView::new("Recipient (name or address):"))
        .child(recipient_view.with_name("recipient"))
        .child(TextView::new("").with_name("recipient_status"))
        .child(TextView::new("Amount:"))
        .child(EditView::new().with_name("amount"))
        .child(create_unit_layout(unit))
}

/// Create the layout for selecting the transaction unit (BTC orSats).
fn create_unit_layout(unit: Arc<Mutex<Unit>>) -> LinearLayout {
    LinearLayout::horizontal()
        .child(TextView::new("Unit: "))
        .child(TextView::new_with_content(TextContent::new("BTC")).with_name("unit_display"))
        .child(Button::new("Switch", move |s| switch_unit(s, unit.clone())))
}

/// Switch the transaction unit between BTC and Sats.
fn switch_unit(s: &mut Cursive, unit: Arc<Mutex<Unit>>) {
    let mut unit = unit.lock().unwrap();
    *unit = match *unit {
        Unit::Btc => Unit::Sats,
        Unit::Sats => Unit::Btc,
    };
    s.call_on_name("unit_display", |view: &mut TextView| {
        view.set_content(match *unit {
            Unit::Btc => "BTC",
            Unit::Sats => "Sats",
        });
    });
}

/// Process the send transaction request.
fn send_transaction(s: &mut Cursive, unit: Unit) {
    debug!("Send button pressed");
    let recipient = s
        .call_on_name("recipient", |view: &mut EditView| view.get_content())
        .unwrap();
    let amount = s
        .call_on_name("amount", |view: &mut EditView| view.get_content())
        .unwrap();
    let amount_decimal =
        BigDecimal::from_str(amount.as_ref()).unwrap_or_else(|_| BigDecimal::from(0u32));
    let amount_sats = convert_amount(&amount_decimal, unit, Unit::Sats)
        .to_u64()
        .unwrap_or(0);

    if amount_sats == 0 {
        show_error_dialog(s, "Amount must be greater than 0");
        return;
    }

    info!(
        "Attempting to send transaction to {} for {} satoshis",
        recipient, amount_sats
    );

    let core = s
        .user_data::<Arc<Core>>()
        .expect("Core missing from user_data")
        .clone();

    // Try to resolve recipient
    let recipient_address = match core.resolve_recipient_address(recipient.as_str()) {
        Ok(addr) => addr,
        Err(e) => {
            show_error_dialog(s, e);
            return;
        }
    };

    // Check if address is not in contacts
    if core.find_contact_by_address(&recipient_address).is_none()
        && core.find_contact_by_name(recipient.as_str()).is_none()
    {
        // Prompt to add as contact
        prompt_add_contact(s, recipient_address.clone(), amount_sats, unit);
    } else {
        // Address is in contacts or was resolved from name, proceed
        proceed_with_transaction(s, &recipient_address, amount_sats);
    }
}

/// Prompt user to add address as contact
fn prompt_add_contact(s: &mut Cursive, address: String, amount: u64, _unit: Unit) {
    s.add_layer(
        Dialog::text(format!(
            "Address '{}' is not in your contacts.\n\nWould you like to add it?",
            address
        ))
        .title("Add Contact?")
        .button("Add Contact", {
            let address = address.clone();
            move |siv| {
                siv.pop_layer();
                show_add_contact_dialog(siv, &address, amount);
            }
        })
        .button("Send Anyway", {
            let address = address.clone();
            move |siv| {
                siv.pop_layer();
                proceed_with_transaction(siv, &address, amount);
            }
        })
        .button("Cancel", |siv| {
            siv.pop_layer();
        }),
    );
}

/// Show dialog to add contact
fn show_add_contact_dialog(s: &mut Cursive, address: &str, amount: u64) {
    let address = address.to_owned();
    let core = s
        .user_data::<Arc<Core>>()
        .expect("Core missing from user_data")
        .clone();
    s.add_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(TextView::new("Contact name:"))
                .child(EditView::new().with_name("contact_name")),
        )
        .title("Add Contact")
        .button("Save", {
            let address = address.clone();
            move |siv| {
                let name = siv
                    .call_on_name("contact_name", |view: &mut EditView| view.get_content())
                    .unwrap();

                if name.trim().is_empty() {
                    show_error_dialog(siv, "Contact name cannot be empty");
                    return;
                }

                match core.add_contact(name.trim().to_string(), address.to_string()) {
                    Ok(_) => {
                        siv.pop_layer();
                        proceed_with_transaction(siv, &address, amount);
                    }
                    Err(e) => {
                        show_error_dialog(siv, format!("{}", e));
                    }
                }
            }
        })
        .button("Cancel", {
            let address = address.clone();
            move |siv| {
                siv.pop_layer();
                proceed_with_transaction(siv, &address, amount);
            }
        }),
    );
}

/// Proceed with transaction after contact handling
fn proceed_with_transaction(s: &mut Cursive, address: &str, amount: u64) {
    let core = s
        .user_data::<Arc<Core>>()
        .expect("Core missing from user_data")
        .clone();
    match core.send_transaction_async(&address, amount) {
        Ok(_) => {
            show_success_dialog(s, "Transaction sent successfully".to_string());
        }
        Err(e) => show_error_dialog(s, format!("{}", e)),
    }
}

/// Display a success dialog after a successful transaction.
fn show_success_dialog(s: &mut Cursive, message: String) {
    let is_transaction = message.contains("Transaction");
    info!("{}", message);
    s.add_layer(
        Dialog::text(message.clone())
            .title("Success")
            .button("OK", move |s| {
                s.pop_layer(); // Close success dialog
                if is_transaction {
                    // Close the transaction dialog that's still on the stack
                    s.pop_layer();
                }
            }),
    );
}

/// Display an error dialog when a transaction fails.
fn show_error_dialog(s: &mut Cursive, error: impl std::fmt::Display) {
    error!("Failed to send transaction: {}", error);
    s.add_layer(
        Dialog::text(format!("Failed to send transaction: {}", error))
            .title("Error")
            .button("OK", |s| {
                debug!("Closing error dialog");
                s.pop_layer();
            }),
    );
}

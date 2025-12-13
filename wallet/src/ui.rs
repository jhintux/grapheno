use crate::core::Core;
use anyhow::Result;
use cursive::event::{Event, Key};
use cursive::traits::*;
use cursive::views::{
    Button, Dialog, EditView, LinearLayout, ListView, Panel, ResizedView, TextContent, TextView,
};
use cursive::Cursive;
use bigdecimal::{BigDecimal, ToPrimitive};
use std::sync::{Arc, Mutex};
use std::str::FromStr;
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
    setup_menubar(siv, core.clone());
    setup_layout(siv, core, balance_content);
    siv.add_global_callback(Event::Key(Key::Esc), |siv| siv.select_menubar());
    siv.select_menubar();
}

/// Show contacts management dialog
fn show_contacts_dialog(s: &mut Cursive, core: Arc<Core>) {
    let config = core.config.read().unwrap();
    let mut list_view = ListView::new();
    
    if config.contacts.is_empty() {
        list_view.add_child("", TextView::new("(No contacts)"));
    } else {
        for contact in &config.contacts {
            let contact_name = contact.name.clone();
            let contact_address = contact.address.clone();
            let core_clone = core.clone();
            
            list_view.add_child(
                &contact.name,
                LinearLayout::horizontal()
                    .child(TextView::new(&format!("{} - {}", contact.name, contact.address)))
                    .child(Button::new("Delete", move |siv| {
                        delete_contact(siv, core_clone.clone(), contact_name.clone(), contact_address.clone());
                    })),
            );
        }
    }
    drop(config);
    
    let core_clone = core.clone();
    s.add_layer(
        Dialog::around(list_view)
            .title("Contacts")
            .button("Add Contact", move |siv| {
                siv.pop_layer();
                show_add_contact_standalone(siv, core_clone.clone());
            })
            .button("Close", |siv| {
                siv.pop_layer();
            }),
    );
}

/// Delete a contact with confirmation
fn delete_contact(s: &mut Cursive, core: Arc<Core>, name: String, address: String) {
    let core_clone = core.clone();
    let name_clone = name.clone();
    s.add_layer(
        Dialog::text(format!("Are you sure you want to delete contact '{}'?\nAddress: {}", name, address))
            .title("Delete Contact")
            .button("Delete", move |siv| {
                match core_clone.remove_contact(&name_clone) {
                    Ok(_) => {
                        siv.pop_layer(); // Close confirmation dialog
                        siv.pop_layer(); // Close contacts dialog
                        show_contacts_dialog(siv, core_clone.clone()); // Refresh contacts dialog
                        show_success_dialog(siv, format!("Contact '{}' deleted successfully", name_clone));
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

/// Show dialog to add contact (standalone, not from transaction)
fn show_add_contact_standalone(s: &mut Cursive, core: Arc<Core>) {
    let core_clone = core.clone();
    s.add_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(TextView::new("Contact name:"))
                .child(EditView::new().with_name("contact_name"))
                .child(TextView::new("Bitcoin address:"))
                .child(EditView::new().with_name("contact_address"))
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
            
            match core_clone.add_contact(name.trim().to_string(), address.trim().to_string()) {
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
fn setup_menubar(siv: &mut Cursive, core: Arc<Core>) {
    let core_clone1 = core.clone();
    let core_clone2 = core.clone();
    siv.menubar()
        .add_leaf("Send", move |s| show_send_transaction(s, core_clone1.clone()))
        .add_leaf("Contacts", move |s| show_contacts_dialog(s, core_clone2.clone()))
        .add_leaf("Quit", |s| s.quit());
    siv.set_autohide_menu(false);
}

/// Set up the main layout of the application.
fn setup_layout(siv: &mut Cursive, core: Arc<Core>, balance_content: TextContent) {
    let instruction = TextView::new("Press Escape to select the top menu");
    let balance_panel = Panel::new(TextView::new_with_content(balance_content)).title("Balance");
    let core_clone = core.clone();
    let info_layout = create_info_layout(&core_clone);
    let layout = LinearLayout::vertical()
        .child(instruction)
        .child(balance_panel)
        .child(info_layout);
    siv.add_layer(layout);
    //siv.add_fullscreen_layer(layout);
}

/// Create the information layout containing keys and contacts.
fn create_info_layout(core: &Arc<Core>) -> LinearLayout {
    let mut info_layout = LinearLayout::horizontal();
    let config = core.config.read().unwrap();
    
    let keys_content = if config.my_keys.is_empty() {
        "(No keys configured)".to_string()
    } else {
        config
            .my_keys
            .iter()
            .map(|key| format!("{}", key.private.display()))
            .collect::<Vec<String>>()
            .join("\n")
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
            .map(|contact| contact.name.clone())
            .collect::<Vec<String>>()
            .join("\n")
    };
    info_layout.add_child(ResizedView::with_full_width(
        Panel::new(TextView::new(contacts_content)).title("Contacts"),
    ));
    info_layout
}

/// Display the send transaction dialog.
fn show_send_transaction(s: &mut Cursive, core: Arc<Core>) {
    info!("Showing send transaction dialog");
    let unit = Arc::new(Mutex::new(Unit::Btc));
    s.add_layer(
        Dialog::around(create_transaction_layout(unit.clone()))
            .title("Send Transaction")
            .button("Send", move |siv| {
                send_transaction(siv, core.clone(), *unit.lock().unwrap())
            })
            .button("Cancel", |siv| {
                debug!("Transaction cancelled");
                siv.pop_layer();
            }),
    );
}

/// Create the layout for the transaction dialog.
fn create_transaction_layout(unit: Arc<Mutex<Unit>>) -> LinearLayout {
    LinearLayout::vertical()
        .child(TextView::new("Recipient (name or address):"))
        .child(EditView::new().with_name("recipient"))
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
fn send_transaction(s: &mut Cursive, core: Arc<Core>, unit: Unit) {
    debug!("Send button pressed");
    let recipient = s
        .call_on_name("recipient", |view: &mut EditView| view.get_content())
        .unwrap();
    let amount = s
        .call_on_name("amount", |view: &mut EditView| view.get_content())
        .unwrap();
    let amount_decimal = BigDecimal::from_str(amount.as_ref()).unwrap_or_else(|_| BigDecimal::from(0u32));
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
    
    // Try to resolve recipient
    let recipient_address = match core.resolve_recipient_address(recipient.as_str()) {
        Ok(addr) => addr,
        Err(e) => {
            show_error_dialog(s, e);
            return;
        }
    };
    
    // Check if address is not in contacts
    if core.find_contact_by_address(&recipient_address).is_none() && 
       core.find_contact_by_name(recipient.as_str()).is_none() {
        // Prompt to add as contact
        prompt_add_contact(s, core.clone(), recipient_address.clone(), amount_sats, unit);
    } else {
        // Address is in contacts or was resolved from name, proceed
        proceed_with_transaction(s, core, recipient_address, amount_sats);
    }
}

/// Prompt user to add address as contact
fn prompt_add_contact(s: &mut Cursive, core: Arc<Core>, address: String, amount: u64, _unit: Unit) {
    let address_clone1 = address.clone();
    let address_clone2 = address.clone();
    let core_clone1 = core.clone();
    let core_clone2 = core.clone();
    s.add_layer(
        Dialog::text(format!("Address '{}' is not in your contacts.\n\nWould you like to add it?", address))
            .title("Add Contact?")
            .button("Add Contact", move |siv| {
                siv.pop_layer();
                show_add_contact_dialog(siv, core_clone1.clone(), address_clone1.clone(), amount);
            })
            .button("Send Anyway", move |siv| {
                siv.pop_layer();
                proceed_with_transaction(siv, core_clone2.clone(), address_clone2.clone(), amount);
            })
            .button("Cancel", |siv| {
                siv.pop_layer();
            }),
    );
}

/// Show dialog to add contact
fn show_add_contact_dialog(s: &mut Cursive, core: Arc<Core>, address: String, amount: u64) {
    let address_clone1 = address.clone();
    let address_clone2 = address.clone();
    let core_clone1 = core.clone();
    let core_clone2 = core.clone();
    s.add_layer(
        Dialog::around(
            LinearLayout::vertical()
                .child(TextView::new("Contact name:"))
                .child(EditView::new().with_name("contact_name"))
        )
        .title("Add Contact")
        .button("Save", move |siv| {
            let name = siv
                .call_on_name("contact_name", |view: &mut EditView| view.get_content())
                .unwrap();
            
            if name.trim().is_empty() {
                show_error_dialog(siv, "Contact name cannot be empty");
                return;
            }
            
            match core_clone1.add_contact(name.trim().to_string(), address_clone1.clone()) {
                Ok(_) => {
                    siv.pop_layer();
                    proceed_with_transaction(siv, core_clone1.clone(), address_clone1.clone(), amount);
                }
                Err(e) => {
                    show_error_dialog(siv, format!("{}", e));
                }
            }
        })
        .button("Cancel", move |siv| {
            siv.pop_layer();
            proceed_with_transaction(siv, core_clone2.clone(), address_clone2.clone(), amount);
        }),
    );
}

/// Proceed with transaction after contact handling
fn proceed_with_transaction(s: &mut Cursive, core: Arc<Core>, address: String, amount: u64) {
    match core.send_transaction_async(&address, amount) {
        Ok(_) => {
            s.pop_layer(); // Close transaction dialog
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
                debug!("Closing success dialog");
                s.pop_layer();
                if is_transaction {
                    s.pop_layer(); // Also close transaction dialog
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

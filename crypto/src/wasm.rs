//! The browser-facing API. Errors become JavaScript exceptions.

use js_sys::{Array, Uint8Array};
use wasm_bindgen::prelude::*;

use crate::device::{Device, Error, Processed};

fn js(error: Error) -> JsError {
    JsError::new(&error.0)
}

fn bytes_array(items: Vec<Vec<u8>>) -> Array {
    items
        .iter()
        .map(|b| JsValue::from(Uint8Array::from(b.as_slice())))
        .collect()
}

#[wasm_bindgen(js_name = Device)]
pub struct WasmDevice(Device);

#[wasm_bindgen(js_name = Commit)]
pub struct WasmCommit {
    commit: Vec<u8>,
    welcome: Option<Vec<u8>>,
}

#[wasm_bindgen(js_class = Commit)]
impl WasmCommit {
    #[wasm_bindgen(getter)]
    pub fn commit(&self) -> Vec<u8> {
        self.commit.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn welcome(&self) -> Option<Vec<u8>> {
        self.welcome.clone()
    }
}

/// Result of processing an incoming message. `sender` and `content` are set
/// for chat messages and absent for membership changes.
#[wasm_bindgen(js_name = Processed)]
pub struct WasmProcessed {
    sender: Option<String>,
    content: Option<Vec<u8>>,
}

#[wasm_bindgen(js_class = Processed)]
impl WasmProcessed {
    #[wasm_bindgen(getter)]
    pub fn sender(&self) -> Option<String> {
        self.sender.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn content(&self) -> Option<Vec<u8>> {
        self.content.clone()
    }
}

#[wasm_bindgen(js_class = Device)]
impl WasmDevice {
    pub fn create(identity: &str) -> Result<WasmDevice, JsError> {
        Device::create(identity).map(WasmDevice).map_err(js)
    }

    pub fn restore(state: &[u8]) -> Result<WasmDevice, JsError> {
        Device::restore(state).map(WasmDevice).map_err(js)
    }

    #[wasm_bindgen(js_name = exportState)]
    pub fn export_state(&self) -> Result<Vec<u8>, JsError> {
        self.0.export_state().map_err(js)
    }

    #[wasm_bindgen(getter)]
    pub fn identity(&self) -> String {
        self.0.identity().to_owned()
    }

    #[wasm_bindgen(js_name = signaturePublicKey)]
    pub fn signature_public_key(&self) -> Vec<u8> {
        self.0.signature_public_key()
    }

    /// Returns an array of `Uint8Array`.
    #[wasm_bindgen(js_name = keyPackages)]
    pub fn key_packages(&self, count: usize, last_resort: bool) -> Result<Array, JsError> {
        self.0
            .key_packages(count, last_resort)
            .map(bytes_array)
            .map_err(js)
    }

    #[wasm_bindgen(js_name = createChat)]
    pub fn create_chat(&self, chat_id: &str) -> Result<(), JsError> {
        self.0.create_chat(chat_id).map_err(js)
    }

    #[wasm_bindgen(js_name = hasChat)]
    pub fn has_chat(&self, chat_id: &str) -> bool {
        self.0.has_chat(chat_id)
    }

    #[wasm_bindgen(js_name = deleteChat)]
    pub fn delete_chat(&self, chat_id: &str) -> Result<(), JsError> {
        self.0.delete_chat(chat_id).map_err(js)
    }

    /// The epoch as a plain number (it stays far below 2^53).
    pub fn epoch(&self, chat_id: &str) -> Result<f64, JsError> {
        self.0.epoch(chat_id).map(|e| e as f64).map_err(js)
    }

    pub fn members(&self, chat_id: &str) -> Result<Vec<String>, JsError> {
        self.0.members(chat_id).map_err(js)
    }

    /// `add_identities[i]` is the identity the server claims for `add_key_packages[i]`.
    #[wasm_bindgen(js_name = changeMembers)]
    pub fn change_members(
        &self,
        chat_id: &str,
        add_identities: Vec<String>,
        add_key_packages: Array,
        remove: Vec<String>,
    ) -> Result<WasmCommit, JsError> {
        if add_identities.len() != add_key_packages.length() as usize {
            return Err(JsError::new("each key package needs exactly one identity"));
        }
        let add: Vec<(String, Vec<u8>)> = add_identities
            .into_iter()
            .zip(add_key_packages.iter().map(|v| Uint8Array::new(&v).to_vec()))
            .collect();
        let commit = self.0.change_members(chat_id, &add, &remove).map_err(js)?;
        Ok(WasmCommit {
            commit: commit.commit,
            welcome: commit.welcome,
        })
    }

    #[wasm_bindgen(js_name = confirmCommit)]
    pub fn confirm_commit(&self, chat_id: &str) -> Result<(), JsError> {
        self.0.confirm_commit(chat_id).map_err(js)
    }

    #[wasm_bindgen(js_name = discardCommit)]
    pub fn discard_commit(&self, chat_id: &str) -> Result<(), JsError> {
        self.0.discard_commit(chat_id).map_err(js)
    }

    /// Returns the id of the chat that was joined.
    #[wasm_bindgen(js_name = joinChat)]
    pub fn join_chat(&self, welcome: &[u8]) -> Result<String, JsError> {
        self.0.join_chat(welcome).map_err(js)
    }

    pub fn encrypt(&self, chat_id: &str, content: &[u8]) -> Result<Vec<u8>, JsError> {
        self.0.encrypt(chat_id, content).map_err(js)
    }

    pub fn process(&self, chat_id: &str, message: &[u8]) -> Result<WasmProcessed, JsError> {
        match self.0.process(chat_id, message).map_err(js)? {
            Processed::Message { sender, content } => Ok(WasmProcessed {
                sender: Some(sender),
                content: Some(content),
            }),
            Processed::MembershipChanged => Ok(WasmProcessed {
                sender: None,
                content: None,
            }),
        }
    }

    #[wasm_bindgen(js_name = exportSecret)]
    pub fn export_secret(&self, chat_id: &str, label: &str, length: usize) -> Result<Vec<u8>, JsError> {
        self.0.export_secret(chat_id, label, length).map_err(js)
    }
}

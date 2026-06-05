use tonic::transport::{Channel, ClientTlsConfig, Certificate, Identity};
use remnanode_proto::xray::app::proxyman::command::{
    handler_service_client::HandlerServiceClient,
    AlterInboundRequest,
    GetInboundUserRequest,
    AddUserOperation,
    RemoveUserOperation,
};
use remnanode_proto::xray::common::serial::TypedMessage;
use remnanode_proto::xray::common::protocol::User;
use prost::Message as ProstMessage;

pub struct HandlerClient {
    inner: HandlerServiceClient<Channel>,
}

impl HandlerClient {
    pub async fn connect(
        addr: &str,
        ca_cert: &[u8],
        client_cert: &[u8],
        client_key: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(ca_cert))
            .identity(Identity::from_pem(client_cert, client_key))
            .domain_name("internal.remnawave.local");

        let channel = Channel::from_shared(format!("http://{addr}"))?
            .tls_config(tls)?
            .connect()
            .await?;

        Ok(Self {
            inner: HandlerServiceClient::new(channel),
        })
    }

    pub async fn alter_inbound_add_user(
        &mut self,
        tag: &str,
        user: User,
    ) -> Result<(), String> {
        let operation = TypedMessage {
            r#type: "type.googleapis.com/xray.app.proxyman.command.AddUserOperation".to_string(),
            value: AddUserOperation { user: Some(user) }.encode_to_vec(),
        };

        let request = AlterInboundRequest {
            tag: tag.to_string(),
            operation: Some(operation),
        };

        self.inner.alter_inbound(request)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn alter_inbound_remove_user(
        &mut self,
        tag: &str,
        email: &str,
    ) -> Result<(), String> {
        let operation = TypedMessage {
            r#type: "type.googleapis.com/xray.app.proxyman.command.RemoveUserOperation".to_string(),
            value: RemoveUserOperation { email: email.to_string() }.encode_to_vec(),
        };

        let request = AlterInboundRequest {
            tag: tag.to_string(),
            operation: Some(operation),
        };

        self.inner.alter_inbound(request)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn get_inbound_users(
        &mut self,
        tag: &str,
    ) -> Result<Vec<User>, String> {
        let request = GetInboundUserRequest {
            tag: tag.to_string(),
            ..Default::default()
        };

        let response = self.inner.get_inbound_users(request)
            .await
            .map_err(|e| e.to_string())?;

        Ok(response.into_inner().users)
    }

    pub async fn get_inbound_users_count(
        &mut self,
        tag: &str,
    ) -> Result<i64, String> {
        let request = GetInboundUserRequest {
            tag: tag.to_string(),
            ..Default::default()
        };

        let response = self.inner.get_inbound_users_count(request)
            .await
            .map_err(|e| e.to_string())?;

        Ok(response.into_inner().count)
    }

    pub async fn remove_outbound(
        &mut self,
        tag: &str,
    ) -> Result<(), String> {
        use remnanode_proto::xray::app::proxyman::command::RemoveOutboundRequest;
        let request = RemoveOutboundRequest { tag: tag.to_string() };
        self.inner.remove_outbound(request)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

// Simple protobuf wire format encoder
mod wire {
    pub fn write_varint(buf: &mut Vec<u8>, value: u64) {
        let mut v = value;
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                buf.push(byte);
                break;
            }
            buf.push(byte | 0x80);
        }
    }

    pub fn write_tag(buf: &mut Vec<u8>, field: u32, wire_type: u8) {
        write_varint(buf, ((field as u64) << 3) | wire_type as u64);
    }

    pub fn write_string(buf: &mut Vec<u8>, field: u32, value: &str) {
        write_tag(buf, field, 2);
        write_varint(buf, value.len() as u64);
        buf.extend_from_slice(value.as_bytes());
    }

    pub fn write_bool(buf: &mut Vec<u8>, field: u32, value: bool) {
        write_tag(buf, field, 0);
        buf.push(if value { 1 } else { 0 });
    }

    pub fn write_int32(buf: &mut Vec<u8>, field: u32, value: i32) {
        write_tag(buf, field, 0);
        write_varint(buf, value as u64);
    }
}

pub mod accounts {
    use super::wire;
    use remnanode_proto::xray::common::protocol::User;
    use remnanode_proto::xray::common::serial::TypedMessage;

    pub fn vless_user(username: &str, uuid: &str, flow: &str) -> User {
        let mut account_bytes = Vec::new();
        wire::write_string(&mut account_bytes, 1, uuid);
        if !flow.is_empty() {
            wire::write_string(&mut account_bytes, 2, flow);
        }

        let account = TypedMessage {
            r#type: "type.googleapis.com/xray.proxy.vless.Account".to_string(),
            value: account_bytes,
        };

        User {
            email: username.to_string(),
            level: 0,
            account: Some(account),
        }
    }

    pub fn trojan_user(username: &str, password: &str) -> User {
        let mut account_bytes = Vec::new();
        wire::write_string(&mut account_bytes, 1, password);

        let account = TypedMessage {
            r#type: "type.googleapis.com/xray.proxy.trojan.Account".to_string(),
            value: account_bytes,
        };

        User {
            email: username.to_string(),
            level: 0,
            account: Some(account),
        }
    }

    pub fn shadowsocks_user(username: &str, password: &str) -> User {
        let mut account_bytes = Vec::new();
        wire::write_int32(&mut account_bytes, 1, 0);
        wire::write_string(&mut account_bytes, 2, password);
        wire::write_bool(&mut account_bytes, 3, false);

        let account = TypedMessage {
            r#type: "type.googleapis.com/xray.proxy.shadowsocks.Account".to_string(),
            value: account_bytes,
        };

        User {
            email: username.to_string(),
            level: 0,
            account: Some(account),
        }
    }

    pub fn shadowsocks_2022_user(username: &str, key_b64: &str) -> User {
        let mut account_bytes = Vec::new();
        wire::write_string(&mut account_bytes, 1, key_b64);

        let account = TypedMessage {
            r#type: "type.googleapis.com/xray.proxy.shadowsocks_2022.Account".to_string(),
            value: account_bytes,
        };

        User {
            email: username.to_string(),
            level: 0,
            account: Some(account),
        }
    }

    pub fn hysteria_user(username: &str, password: &str) -> User {
        let mut account_bytes = Vec::new();
        wire::write_string(&mut account_bytes, 1, password);

        let account = TypedMessage {
            r#type: "type.googleapis.com/xray.proxy.hysteria.Account".to_string(),
            value: account_bytes,
        };

        User {
            email: username.to_string(),
            level: 0,
            account: Some(account),
        }
    }
}

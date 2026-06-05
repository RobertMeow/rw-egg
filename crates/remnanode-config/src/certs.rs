use rcgen::{CertificateParams, DnType, KeyPair, IsCa, BasicConstraints, KeyUsagePurpose, ExtendedKeyUsagePurpose, SanType};

#[derive(Debug, Clone)]
pub struct MtlsCerts {
    pub ca_cert_pem: String,
    pub ca_key_pem: String,
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

pub fn generate_mtls_certs() -> MtlsCerts {
    // CA certificate
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name.push(DnType::CommonName, "Remnawave Internal CA");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    let ca_key = KeyPair::generate().expect("Failed to generate CA key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("Failed to sign CA cert");

    // Server certificate
    let mut server_params = CertificateParams::default();
    server_params.distinguished_name.push(DnType::CommonName, "internal.remnawave.local");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.subject_alt_names = vec![
        SanType::DnsName(rcgen::Ia5String::try_from("internal.remnawave.local").unwrap()),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
    ];

    let server_key = KeyPair::generate().expect("Failed to generate server key");
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key)
        .expect("Failed to sign server cert");

    // Client certificate
    let mut client_params = CertificateParams::default();
    client_params.distinguished_name.push(DnType::CommonName, "internal.remnawave.local");
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

    let client_key = KeyPair::generate().expect("Failed to generate client key");
    let client_cert = client_params.signed_by(&client_key, &ca_cert, &ca_key)
        .expect("Failed to sign client cert");

    MtlsCerts {
        ca_cert_pem: ca_cert.pem(),
        ca_key_pem: ca_key.serialize_pem(),
        server_cert_pem: server_cert.pem(),
        server_key_pem: server_key.serialize_pem(),
        client_cert_pem: client_cert.pem(),
        client_key_pem: client_key.serialize_pem(),
    }
}

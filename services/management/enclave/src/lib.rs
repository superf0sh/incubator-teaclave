// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

#![cfg_attr(feature = "mesalock_sgx", no_std)]
#[cfg(feature = "mesalock_sgx")]
#[macro_use]
extern crate sgx_tstd as std;

#[macro_use]
extern crate log;

use std::prelude::v1::*;

use teaclave_attestation::{verifier, AttestationConfig, RemoteAttestation};
use teaclave_binder::proto::{
    ECallCommand, FinalizeEnclaveInput, FinalizeEnclaveOutput, InitEnclaveInput, InitEnclaveOutput,
    StartServiceInput, StartServiceOutput,
};
use teaclave_binder::{handle_ecall, register_ecall_handler};
use teaclave_config::{RuntimeConfig, BUILD_CONFIG};
use teaclave_proto::teaclave_management_service::{
    TeaclaveManagementRequest, TeaclaveManagementResponse,
};
use teaclave_rpc::config::{SgxTrustedTlsClientConfig, SgxTrustedTlsServerConfig};
use teaclave_rpc::endpoint::Endpoint;
use teaclave_rpc::server::SgxTrustedTlsServer;
use teaclave_service_enclave_utils::ServiceEnclave;
use teaclave_types::{EnclaveInfo, TeeServiceError, TeeServiceResult};

mod service;
mod task;

const AS_ROOT_CA_CERT: &[u8] = BUILD_CONFIG.as_root_ca_cert;
const AUDITOR_PUBLIC_KEYS_LEN: usize = BUILD_CONFIG.auditor_public_keys.len();
const AUDITOR_PUBLIC_KEYS: &[&[u8]; AUDITOR_PUBLIC_KEYS_LEN] = BUILD_CONFIG.auditor_public_keys;
const INBOUND_SERVICES_LEN: usize = BUILD_CONFIG.inbound.management.len();
const INBOUND_SERVICES: &[&str; INBOUND_SERVICES_LEN] = BUILD_CONFIG.inbound.management;

fn start_service(config: &RuntimeConfig) -> anyhow::Result<()> {
    let listen_address = config.internal_endpoints.management.listen_address;
    let as_config = &config.attestation;
    let attestation_config = AttestationConfig::new(
        &as_config.algorithm,
        &as_config.url,
        &as_config.key,
        &as_config.spid,
    );
    let attested_tls_config = RemoteAttestation::new()
        .config(attestation_config)
        .generate_and_endorse()
        .unwrap()
        .attested_tls_config()
        .unwrap();
    let enclave_info = EnclaveInfo::verify_and_new(
        config
            .audit
            .enclave_info_bytes
            .as_ref()
            .expect("enclave_info"),
        AUDITOR_PUBLIC_KEYS,
        config
            .audit
            .auditor_signatures_bytes
            .as_ref()
            .expect("auditor signatures"),
    )?;
    let accepted_enclave_attrs: Vec<teaclave_types::EnclaveAttr> = INBOUND_SERVICES
        .iter()
        .map(|service| {
            enclave_info
                .get_enclave_attr(service)
                .expect("enclave_info")
        })
        .collect();
    let server_config = SgxTrustedTlsServerConfig::from_attested_tls_config(attested_tls_config)
        .unwrap()
        .attestation_report_verifier(
            accepted_enclave_attrs,
            AS_ROOT_CA_CERT,
            verifier::universal_quote_verifier,
        )
        .unwrap();
    let mut server =
        SgxTrustedTlsServer::<TeaclaveManagementResponse, TeaclaveManagementRequest>::new(
            listen_address,
            server_config,
        );

    let storage_service_enclave_attrs = enclave_info
        .get_enclave_attr("teaclave_storage_service")
        .expect("enclave_info");
    let storage_service_client_config = SgxTrustedTlsClientConfig::new()
        .attestation_report_verifier(
            vec![storage_service_enclave_attrs],
            AS_ROOT_CA_CERT,
            verifier::universal_quote_verifier,
        );

    let storage_service_address = &config.internal_endpoints.storage.advertised_address;

    let storage_service_endpoint =
        Endpoint::new(storage_service_address).config(storage_service_client_config);

    let service = service::TeaclaveManagementService::new(storage_service_endpoint)?;
    match server.start(service) {
        Ok(_) => (),
        Err(e) => {
            error!("Service exit, error: {}.", e);
        }
    }
    Ok(())
}

#[handle_ecall]
fn handle_start_service(input: &StartServiceInput) -> TeeServiceResult<StartServiceOutput> {
    start_service(&input.config).map_err(|_| TeeServiceError::ServiceError)?;
    Ok(StartServiceOutput)
}

#[handle_ecall]
fn handle_init_enclave(_: &InitEnclaveInput) -> TeeServiceResult<InitEnclaveOutput> {
    ServiceEnclave::init(env!("CARGO_PKG_NAME"))?;
    Ok(InitEnclaveOutput)
}

#[handle_ecall]
fn handle_finalize_enclave(_: &FinalizeEnclaveInput) -> TeeServiceResult<FinalizeEnclaveOutput> {
    ServiceEnclave::finalize()?;
    Ok(FinalizeEnclaveOutput)
}

register_ecall_handler!(
    type ECallCommand,
    (ECallCommand::StartService, StartServiceInput, StartServiceOutput),
    (ECallCommand::InitEnclave, InitEnclaveInput, InitEnclaveOutput),
    (ECallCommand::FinalizeEnclave, FinalizeEnclaveInput, FinalizeEnclaveOutput),
);

#[cfg(feature = "enclave_unit_test")]
pub mod tests {
    use super::*;
    use teaclave_test_utils::*;

    pub fn run_tests() -> bool {
        run_tests!(
            service::tests::handle_input_file,
            service::tests::handle_output_file,
            service::tests::handle_function,
            service::tests::handle_task,
            service::tests::handle_staged_task,
        )
    }
}

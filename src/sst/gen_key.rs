use crate::constants::LmsTreeIdentifier;
use crate::signature::Error;
use crate::{
    constants::{D_INTR, MAX_DSM_SIGNING_ENTITIES, MAX_HASH_SIZE},
    hasher::HashChain,
    hss::{
        definitions::HssPublicKey,
        definitions::HssPrivateKey,
        reference_impl_private_key::{ReferenceImplPrivateKey, Seed},
        SigningKey, VerifyingKey,
        aux::{hss_finalize_aux_data, hss_is_aux_data_used},
    },
    lms::definitions::LmsPrivateKey,
    lms::helper::get_tree_element,
    sst::{
        helper, helper::get_subtree_node_idx, parameters::SstExtension, parameters::SstsParameter,
    },
};

use tinyvec::ArrayVec;

pub fn genkey1_sst<H: HashChain>(
    sst_param: &SstsParameter<H>,
    seed: &Seed<H>,
    aux_data: Option<&mut &mut [u8]>,
) -> Result<(SigningKey<H>, ArrayVec<[u8; MAX_HASH_SIZE]>), Error> {

    // create two representations of private keys because we need their data elements
    // -> ReferenceImplPrivateKey and SigningKey
    let rfc_private_key =
        ReferenceImplPrivateKey::generate(sst_param, seed).map_err(|_| Error::new())?;
    let signing_key = SigningKey::from_bytes(&rfc_private_key.to_binary_representation())?;

    // get expanded AUX data
    let is_aux_data_used = if let Some(ref aux_data) = aux_data {
        hss_is_aux_data_used(aux_data)
    } else {
        false
    };

    let mut expanded_aux_data = HssPrivateKey::get_expanded_aux_data(
        aux_data,
        &rfc_private_key,
        sst_param.get_hss_parameters()[0].get_lms_parameter(), // TODO only top is forwarded to LmsPrivateKey for SST params
        is_aux_data_used,
    );

    // calculate our intermediate node hash value; for this we have to generate a LmsPrivateKey

    // TODO review: better option? redundant (used leafs calculation)
    let mut used_leafs_index = 0;
    if sst_param.get_top_div_height() != 0 {
        // TODO: is there a better (Rust-idiomatic) approach?
        used_leafs_index = helper::get_sst_first_leaf_idx(
            sst_param.get_signing_entity_idx(),
            sst_param.get_hss_parameters()[0]
                .get_lms_parameter()
                .get_tree_height(),
            sst_param.get_top_div_height(),
        );
    }

    // TODO: review! not exactly elegant to create an LmsPrivateKey and SeedAndLmsTreeIdentifier
    let seed_and_lms_tree_ident = rfc_private_key.generate_root_seed_and_lms_tree_identifier();

    let sst_ext = SstExtension {
        signing_entity_idx: rfc_private_key.sst_ext.signing_entity_idx,
        top_div_height: rfc_private_key.sst_ext.top_div_height,
    };

    let mut sst_ext_option = None;
    let mut our_node_index = 1; // TODO don't do that...we're calc. the TOTAL public key

    if rfc_private_key.sst_ext.signing_entity_idx != 0 {
        sst_ext_option = Some(sst_ext);

        our_node_index = get_subtree_node_idx(
            sst_param.get_signing_entity_idx(),
            sst_param.get_hss_parameters()[0]
                .get_lms_parameter()
                .get_tree_height(),
            sst_param.get_top_div_height(),
        );
    }

    let lms_private_key = LmsPrivateKey::<H>::new(
        seed_and_lms_tree_ident.seed.clone(),
        seed_and_lms_tree_ident.lms_tree_identifier,
        used_leafs_index, // actually not used in "get_tree_element", irrelevant
        *sst_param.get_hss_parameters()[0].get_lmots_parameter(),
        *sst_param.get_hss_parameters()[0].get_lms_parameter(),
        sst_ext_option,
    );

    // TODO do this via LmsPublicKey() and have aux data taken care of? where is aux data finalized in original code?
    let our_node_value = get_tree_element(our_node_index as usize, &lms_private_key, &mut expanded_aux_data);
    if let Some(expanded_aux_data) = expanded_aux_data.as_mut() {
        hss_finalize_aux_data::<H>(expanded_aux_data, rfc_private_key.seed.as_slice());
    }

    Ok((signing_key, our_node_value))
}

pub fn get_num_signing_entities<H: HashChain>(
    private_key: &[u8],
) -> Result<u32, Error> {
    let rfc_private_key = ReferenceImplPrivateKey::<H>::from_binary_representation(private_key)
        .map_err(|_| Error::new())?;

    let num_signing_entities = 2u32.pow(rfc_private_key.sst_ext.top_div_height as u32);

    Ok(num_signing_entities)
}

pub fn genkey2_sst<H: HashChain>(
    private_key: &[u8],
    interm_nodes: &ArrayVec<[ArrayVec<[u8; MAX_HASH_SIZE]>; MAX_DSM_SIGNING_ENTITIES]>,
    aux_data: Option<&mut &mut [u8]>
) -> Result<VerifyingKey<H>, Error> {

    let rfc_private_key = ReferenceImplPrivateKey::<H>::from_binary_representation(private_key)
        .map_err(|_| Error::new())?;

    let seed_and_lms_tree_ident = rfc_private_key.generate_root_seed_and_lms_tree_identifier();
    let lms_tree_ident = seed_and_lms_tree_ident.lms_tree_identifier;

    // TODO we don't need AUX here to read from, but we could populate the upper levels; AUX level marker needs then to be updated
    let pubkey_hash_val = get_node_hash_val::<H>(
        1, interm_nodes, rfc_private_key.sst_ext.top_div_height, lms_tree_ident);

    let hss_public_key = HssPublicKey::from_with_sst(
        &rfc_private_key, aux_data, interm_nodes, pubkey_hash_val).map_err(|_| Error::new())?;

    let verifying_key = VerifyingKey::<H>::from_bytes(&hss_public_key.to_binary_representation())?;

    Ok(verifying_key)
}

fn get_node_hash_val<H: HashChain>(
    index: u32,
    av_of_nodes: &ArrayVec<[ArrayVec<[u8; MAX_HASH_SIZE]>; MAX_DSM_SIGNING_ENTITIES]>,
    top_div_height: u8,
    lms_tree_ident: LmsTreeIdentifier,
) -> ArrayVec<[u8; MAX_HASH_SIZE]> {
    let index_level =
        (core::mem::size_of_val(&index) * 8 - index.leading_zeros() as usize - 1) as u8;

    let hasher = H::default()
        .chain(lms_tree_ident)
        .chain((index).to_be_bytes());

    // if index is at lowest level (where we have the signing entity node hash values)
    let result = if index_level == top_div_height {
        // return the node value from array of intermedediate node hash values
        /* access vector elements via "leaf numbers" = 0..signing_entites-1 */
        let leaf_number = (index as usize) - 2usize.pow(top_div_height as u32);
        av_of_nodes[leaf_number]
    } else {
        // we are "above" the intermediate node hash values -> go down
        let left = get_node_hash_val::<H>(index * 2, av_of_nodes, top_div_height, lms_tree_ident);
        let right =
            get_node_hash_val::<H>(index * 2 + 1, av_of_nodes, top_div_height, lms_tree_ident);

        hasher
            .chain(D_INTR)
            .chain(left.as_slice())
            .chain(right.as_slice())
            .finalize()
    };

    result
}

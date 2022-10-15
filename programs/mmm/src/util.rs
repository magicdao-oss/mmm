use crate::{constants::POOL_PREFIX, errors::MMMErrorCode, state::*};
use anchor_lang::prelude::*;
use anchor_spl::token::Mint;
use mpl_token_metadata::{
    id as token_metadata_program_key,
    pda::{find_master_edition_account, find_metadata_account},
    state::{Metadata, TokenMetadataAccount},
};

// copied from mpl-token-metadata
fn check_master_edition(master_edition_account_info: &AccountInfo) -> bool {
    let version = master_edition_account_info.data.borrow()[0];
    return version == 2 || version == 6;
}

pub fn check_allowlists(allowlists: &[Allowlist]) -> Result<()> {
    for allowlist in allowlists.iter() {
        if !allowlist.valid() {
            msg!("InvalidAllowLists: invalid entry");
            return Err(MMMErrorCode::InvalidAllowLists.into());
        }
    }

    Ok(())
}

pub fn check_allowlists_for_mint(
    allowlists: &[Allowlist],
    mint: &Account<Mint>,
    metadata: &AccountInfo,
    master_edition: &AccountInfo,
) -> Result<()> {
    // TODO: we need to check the following validation rules
    // 1. make sure the metadata is correctly derived from the metadata pda with the mint
    // 2. make sure mint+metadata(e.g. first verified creator address) can match one of the allowlist
    // 3. note that the allowlist is unioned together, not intersection
    // 4. skip if the allowlist.is_empty()
    // 5. verify that nft either does not have master edition or is master edition

    if *metadata.owner != token_metadata_program_key() {
        return Err(ErrorCode::AccountOwnedByWrongProgram.into());
    }
    if find_metadata_account(&mint.key()).0 != metadata.key() {
        return Err(ErrorCode::ConstraintSeeds.into());
    }
    if find_master_edition_account(&mint.key()).0 != master_edition.key() {
        return Err(ErrorCode::ConstraintSeeds.into());
    }
    let parsed_metadata = Metadata::from_account_info(metadata)?;
    if !master_edition.data_is_empty() {
        if master_edition.owner.ne(&token_metadata_program_key()) {
            return Err(ErrorCode::AccountOwnedByWrongProgram.into());
        }
        if !check_master_edition(master_edition) {
            return Err(MMMErrorCode::InvalidMasterEdition.into());
        }
    }

    for allowlist_val in allowlists.iter() {
        match allowlist_val.kind {
            ALLOWLIST_KIND_EMPTY => {}
            ALLOWLIST_KIND_FVCA => match parsed_metadata.data.creators {
                Some(ref creators) => {
                    // TODO: can we make sure we only take master_edition here?
                    if !creators.is_empty()
                        && creators[0].address == allowlist_val.value
                        && creators[0].verified
                    {
                        return Ok(());
                    }
                }
                _ => {}
            },
            ALLOWLIST_KIND_MINT => {
                if mint.key() == allowlist_val.value {
                    return Ok(());
                }
            }
            ALLOWLIST_KIND_MCC => match parsed_metadata.collection {
                Some(ref collection_data) => {
                    if collection_data.key == allowlist_val.value && collection_data.verified {
                        return Ok(());
                    }
                }
                _ => {}
            },
            _ => {
                return Err(MMMErrorCode::InvalidAllowLists.into());
            }
        }
    }

    // at the end, we didn't find a match, thus return err
    return Err(MMMErrorCode::InvalidAllowLists.into());
}

pub fn check_curve(curve_type: u8, curve_delta: u64) -> Result<()> {
    // So far we only allow linear and exponential curves
    // 0: linear
    // 1: exp
    if curve_type > 1 {
        return Err(MMMErrorCode::InvalidCurveType.into());
    }

    // If the curve type is exp, then the curve_delta should follow bp format,
    // which is less than 10000
    if curve_type == 1 && curve_delta > 10000 {
        return Err(MMMErrorCode::InvalidCurveDelta.into());
    }

    Ok(())
}

pub fn get_sol_lp_fee(
    pool: &Pool,
    buyside_sol_escrow_balance: u64,
    total_sol_price: u64,
) -> Result<u64> {
    if pool.sellside_orders_count < 1 {
        return Ok(0);
    }

    if buyside_sol_escrow_balance < pool.spot_price {
        return Ok(0);
    }

    Ok(((total_sol_price as u128)
        .checked_mul(pool.lp_fee_bp as u128)
        .ok_or(MMMErrorCode::NumericOverflow)?
        .checked_div(10000)
        .ok_or(MMMErrorCode::NumericOverflow)?) as u64)
}

pub fn get_sol_referral_fee(pool: &Pool, total_sol_price: u64) -> Result<u64> {
    Ok((total_sol_price as u128)
        .checked_mul(pool.referral_bp as u128)
        .ok_or(MMMErrorCode::NumericOverflow)?
        .checked_div(10000)
        .ok_or(MMMErrorCode::NumericOverflow)? as u64)
}

pub fn get_sol_total_price_and_next_price(
    pool: &Pool,
    n: u64,
    fulfill_buy: bool,
) -> Result<(u64, u64)> {
    // the price needs to go down
    let p = pool.spot_price;
    let delta = pool.curve_delta;
    match fulfill_buy {
        true => {
            match pool.curve_type {
                CURVE_KIND_LINEAR => {
                    // n*(2*p-(n-1)*delta)/2
                    let total_price = n
                        .checked_mul(
                            p.checked_mul(2)
                                .ok_or(MMMErrorCode::NumericOverflow)?
                                .checked_sub(
                                    n.checked_sub(1)
                                        .ok_or(MMMErrorCode::NumericOverflow)?
                                        .checked_mul(delta)
                                        .ok_or(MMMErrorCode::NumericOverflow)?,
                                )
                                .ok_or(MMMErrorCode::NumericOverflow)?,
                        )
                        .ok_or(MMMErrorCode::NumericOverflow)?
                        .checked_div(2)
                        .ok_or(MMMErrorCode::NumericOverflow)?;
                    // p - n * delta
                    let final_price = p
                        .checked_sub(n.checked_mul(delta).ok_or(MMMErrorCode::NumericOverflow)?)
                        .ok_or(MMMErrorCode::NumericOverflow)?;
                    Ok((total_price, final_price))
                }
                CURVE_KIND_EXP => {
                    // for loop to prevent overflow
                    let mut total_price: u64 = 0;
                    let mut curr_price: u128 = p as u128;
                    for _ in 0..n {
                        total_price = total_price
                            .checked_add(curr_price as u64)
                            .ok_or(MMMErrorCode::NumericOverflow)?;
                        curr_price = curr_price
                            .checked_mul(10000)
                            .ok_or(MMMErrorCode::NumericOverflow)?
                            .checked_div(
                                (delta as u128)
                                    .checked_add(10000)
                                    .ok_or(MMMErrorCode::NumericOverflow)?,
                            )
                            .ok_or(MMMErrorCode::NumericOverflow)?;
                    }
                    Ok((total_price, curr_price as u64))
                }
                _ => Err(MMMErrorCode::InvalidCurveType.into()),
            }
        }
        false => {
            match pool.curve_type {
                CURVE_KIND_LINEAR => {
                    // n*(2*p+(n-1)*delta)/2
                    let total_price = n
                        .checked_mul(
                            p.checked_mul(2)
                                .ok_or(MMMErrorCode::NumericOverflow)?
                                .checked_add(
                                    n.checked_sub(1)
                                        .ok_or(MMMErrorCode::NumericOverflow)?
                                        .checked_mul(delta)
                                        .ok_or(MMMErrorCode::NumericOverflow)?,
                                )
                                .ok_or(MMMErrorCode::NumericOverflow)?,
                        )
                        .ok_or(MMMErrorCode::NumericOverflow)?
                        .checked_div(2)
                        .ok_or(MMMErrorCode::NumericOverflow)?;
                    // p - n * delta
                    let final_price = p
                        .checked_add(n.checked_mul(delta).ok_or(MMMErrorCode::NumericOverflow)?)
                        .ok_or(MMMErrorCode::NumericOverflow)?;
                    Ok((total_price, final_price))
                }
                CURVE_KIND_EXP => {
                    // r = (1 + delta/10000)
                    // p * (1-(1+r^n)/(1-r))
                    let mut total_price: u64 = 0;
                    let mut curr_price: u128 = p as u128;
                    for _ in 0..n {
                        total_price = total_price
                            .checked_add(curr_price as u64)
                            .ok_or(MMMErrorCode::NumericOverflow)?;
                        curr_price = curr_price
                            .checked_mul(
                                (delta as u128)
                                    .checked_add(10000)
                                    .ok_or(MMMErrorCode::NumericOverflow)?,
                            )
                            .ok_or(MMMErrorCode::NumericOverflow)?
                            .checked_div(10000)
                            .ok_or(MMMErrorCode::NumericOverflow)?;
                    }
                    Ok((total_price, curr_price as u64))
                }
                _ => Err(MMMErrorCode::InvalidCurveType.into()),
            }
        }
    }
}

pub fn try_close_pool<'info>(
    pool: &Account<'info, Pool>,
    pool_bump: u8,
    owner: AccountInfo<'info>,
    system_program: AccountInfo<'info>,
    buyside_sol_escrow_balance: u64,
) -> Result<()> {
    if pool.sellside_orders_count != 0 {
        return Ok(());
    }

    if buyside_sol_escrow_balance != 0 {
        return Ok(());
    }

    anchor_lang::solana_program::program::invoke_signed(
        &anchor_lang::solana_program::system_instruction::transfer(
            &pool.key(),
            &pool.owner.key(),
            pool.to_account_info().lamports(),
        ),
        &[
            pool.to_account_info(),
            owner.to_account_info(),
            system_program.to_account_info(),
        ],
        // seeds should be the PDA of 'pool'
        &[&[
            POOL_PREFIX.as_bytes(),
            owner.key().as_ref(),
            pool.uuid.key().as_ref(),
            &[pool_bump],
        ]],
    )?;

    pool.to_account_info()
        .data
        .borrow_mut()
        .copy_from_slice(&[0; Pool::LEN]);

    Ok(())
}

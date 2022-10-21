import * as anchor from '@project-serum/anchor';
import { Program } from '@project-serum/anchor';
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  getAccount as getTokenAccount,
  getAssociatedTokenAddress,
  TOKEN_PROGRAM_ID,
} from '@solana/spl-token';
import {
  Keypair,
  LAMPORTS_PER_SOL,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
} from '@solana/web3.js';
import { assert } from 'chai';
import { Mmm, AllowlistKind, CurveKind, getMMMSellStatePDA } from '../sdk/src';
import {
  assertIsBetween,
  createPoolWithExampleDeposits,
  LAMPORT_ERROR_RANGE,
  SIGNATURE_FEE_LAMPORTS,
} from './utils';

describe('mmm-withdraw', () => {
  const { wallet, connection, opts } = anchor.AnchorProvider.env();
  opts.commitment = 'processed';
  const program = anchor.workspace.Mmm as Program<Mmm>;
  const cosigner = Keypair.generate();

  it('Withdraw payment', async () => {
    const poolData = await createPoolWithExampleDeposits(
      program,
      connection,
      AllowlistKind.fvca,
      {
        owner: wallet.publicKey,
        cosigner,
        curveType: CurveKind.linear,
        curveDelta: new anchor.BN(LAMPORTS_PER_SOL).div(new anchor.BN(10)), // 0.1 SOL
        expiry: new anchor.BN(new Date().getTime() / 1000 + 1000),
        reinvestFulfillBuy: true,
        reinvestFulfillSell: true,
      },
      'buy',
    );

    let initWalletBalance = await connection.getBalance(wallet.publicKey);
    await program.methods
      .solWithdrawBuy({ paymentAmount: new anchor.BN(6 * LAMPORTS_PER_SOL) })
      .accountsStrict({
        owner: wallet.publicKey,
        cosigner: cosigner.publicKey,
        pool: poolData.poolKey,
        buysideSolEscrowAccount: poolData.poolPaymentEscrow,
        systemProgram: SystemProgram.programId,
      })
      .signers([cosigner])
      .rpc();

    {
      const poolAccountInfo = await program.account.pool.fetch(
        poolData.poolKey,
      );
      assert.equal(
        poolAccountInfo.buysidePaymentAmount.toNumber(),
        4 * LAMPORTS_PER_SOL,
      );
      const walletBalance = await connection.getBalance(wallet.publicKey);
      assertIsBetween(
        walletBalance,
        initWalletBalance + 6 * LAMPORTS_PER_SOL - 2 * SIGNATURE_FEE_LAMPORTS,
        LAMPORT_ERROR_RANGE,
      );
      initWalletBalance = walletBalance;
    }

    await program.methods
      .solWithdrawBuy({ paymentAmount: new anchor.BN(4 * LAMPORTS_PER_SOL) })
      .accountsStrict({
        owner: wallet.publicKey,
        cosigner: cosigner.publicKey,
        pool: poolData.poolKey,
        buysideSolEscrowAccount: poolData.poolPaymentEscrow,
        systemProgram: SystemProgram.programId,
      })
      .signers([cosigner])
      .rpc();

    {
      assert.equal(await connection.getBalance(poolData.poolKey), 0);
      const walletBalance = await connection.getBalance(wallet.publicKey);
      assert.isAtLeast(
        walletBalance,
        initWalletBalance + 4 * LAMPORTS_PER_SOL - 2 * SIGNATURE_FEE_LAMPORTS,
      );
    }
  });

  it('Withdraw assets', async () => {
    const poolData = await createPoolWithExampleDeposits(
      program,
      connection,
      AllowlistKind.mint,
      {
        owner: wallet.publicKey,
        cosigner,
        curveType: CurveKind.linear,
        curveDelta: new anchor.BN(LAMPORTS_PER_SOL).div(new anchor.BN(10)), // 0.1 SOL
        expiry: new anchor.BN(new Date().getTime() / 1000 + 1000),
        reinvestFulfillBuy: true,
        reinvestFulfillSell: true,
      },
      'sell',
    );

    const ownerNftAtaAddress = await getAssociatedTokenAddress(
      poolData.nft.mintAddress,
      wallet.publicKey,
    );
    const { key: nftSellState } = getMMMSellStatePDA(
      program.programId,
      poolData.poolKey,
      poolData.nft.mintAddress,
    );
    await program.methods
      .withdrawSell({ assetAmount: new anchor.BN(1), allowlistAux: null })
      .accountsStrict({
        owner: wallet.publicKey,
        cosigner: cosigner.publicKey,
        pool: poolData.poolKey,
        assetMint: poolData.nft.mintAddress,
        assetTokenAccount: ownerNftAtaAddress,
        sellsideEscrowTokenAccount: poolData.poolAtaNft,
        buysideSolEscrowAccount: poolData.poolPaymentEscrow,
        allowlistAuxAccount: SystemProgram.programId,
        sellState: nftSellState,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([cosigner])
      .rpc();

    const ownerNftAta = await getTokenAccount(connection, ownerNftAtaAddress);
    assert.equal(Number(ownerNftAta.amount), 1);
    assert.equal(ownerNftAta.owner.toBase58(), wallet.publicKey.toBase58());
  });
});
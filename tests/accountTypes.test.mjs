import assert from "node:assert/strict";
import test from "node:test";
import { amountIsMoneyIn, signedAmountCents } from "../src/lib/accountTypes.ts";

// ADR-0004: positive amount_cents always means "this account's own balance
// went up." For an asset, that's the everyday meaning of "money in." For a
// liability, a charge/purchase increases what's owed (positive) even
// though the user experiences it as spending, and a payment/credit
// decreases it (negative) even though it reads like money arriving.

test("asset accounts: money in is positive, money out is negative", () => {
  assert.equal(signedAmountCents(8422, true, "checking"), 8422);
  assert.equal(signedAmountCents(8422, false, "checking"), -8422);
  assert.equal(signedAmountCents(500, true, "savings"), 500);
  assert.equal(signedAmountCents(500, false, "cash"), -500);
});

test("liability accounts invert: a charge is positive, a payment/credit is negative", () => {
  // A $84.22 restaurant charge on a credit card increases what's owed.
  assert.equal(signedAmountCents(8422, false, "credit_card"), 8422);
  // A $500 payment toward the card, or a merchant refund/credit, decreases
  // what's owed.
  assert.equal(signedAmountCents(50000, true, "credit_card"), -50000);
  assert.equal(signedAmountCents(3000, true, "loan"), -3000);
  assert.equal(signedAmountCents(3000, false, "other_liability"), 3000);
});

test("amountIsMoneyIn is the exact inverse of signedAmountCents for every account type", () => {
  const accountTypes = [
    "checking",
    "savings",
    "cash",
    "credit_card",
    "loan",
    "other_liability",
  ];
  for (const accountType of accountTypes) {
    for (const moneyIn of [true, false]) {
      const signed = signedAmountCents(4200, moneyIn, accountType);
      assert.equal(
        amountIsMoneyIn(signed, accountType),
        moneyIn,
        `round-trip failed for ${accountType}/${moneyIn}`,
      );
    }
  }
});

test("amountIsMoneyIn matches the manual/imported convergence example: a real imported credit-card purchase reads as a charge, not a payment", () => {
  // The same convention CSV/OFX already use (money.rs, import.rs): a
  // positive amount on a liability account is a purchase/charge.
  assert.equal(amountIsMoneyIn(8422, "credit_card"), false);
  // A negative amount on a liability account is a payment/credit.
  assert.equal(amountIsMoneyIn(-50000, "credit_card"), true);
});

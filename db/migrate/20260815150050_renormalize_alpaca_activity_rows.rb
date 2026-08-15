# One-off repair: Alpaca activity rows imported before this branch carry the entry types the OLD
# normalizer gave them, and `ResetAlpacaSyncWatermarks` (the next migration) cannot heal them —
# `AccountTransactionSync` skips a re-fetched row whose tx_id is already stored, it never updates.
#
# Left alone, the German broker report reads a legacy dividend as bank interest (Zeile 19, no
# Teilfreistellung), adds a withholding DEBIT to that income while losing its Zeile 41 credit, misses
# a return of capital's basis reduction entirely, and `Tax::CryptoScope` cannot match a crypto fill
# whose base_currency is still a pair string, so those disposals fall out of the crypto report too.
#
# Restated from `raw_data`, which is the verbatim Alpaca activity and therefore the source of truth
# regardless of which normalizer wrote the row. Idempotent: re-running produces the same values, and
# a row the new normalizer already wrote is unchanged.
#
# Restated rather than deleted-and-re-imported on purpose. Deletion leans on the re-pull that
# `ResetAlpacaSyncWatermarks` sets up, and that re-pull never happens for a user who has since
# disconnected Alpaca — their whole dividend history would simply vanish from a tax document. These
# four families are also never `linked_transaction_id` targets (only deposits and withdrawals are),
# so there is nothing a rewrite can dangle.
#
# Raw SQL only, no models or callbacks.
class RenormalizeAlpacaActivityRows < ActiveRecord::Migration[8.1]
  # AccountTransaction.entry_types — spelled out because the enum may be renumbered later.
  OTHER_INCOME = 11
  WITHHOLDING_TAX = 13
  RETURN_OF_CAPITAL = 14

  DIVIDEND_INCOME_TYPES = %w[DIV DIVCGL DIVCGS CGD DIVTXEX].freeze
  WITHHOLDING_TYPES = %w[DIVNRA DIVFT DIVTW INTNRA INTTW].freeze
  INTEREST_TYPES = %w[INT PTR].freeze

  def up
    # A dividend's security is read off quote_currency; the old mapping left it nil.
    execute(<<~SQL.squish)
      UPDATE account_transactions
      SET entry_type = #{OTHER_INCOME},
          base_currency = 'USD',
          quote_currency = json_extract(raw_data, '$.symbol'),
          quote_amount = NULL,
          description = 'Dividend (' || json_extract(raw_data, '$.symbol') || ')'
      WHERE #{alpaca_rows} AND #{activity_type_in(DIVIDEND_INCOME_TYPES)}
    SQL

    # Foreign tax withheld is a credit, not income. Stored as other_income it was ADDED to Zeile 19
    # and its Zeile 41 credit was lost — wrong in both directions.
    execute(<<~SQL.squish)
      UPDATE account_transactions
      SET entry_type = #{WITHHOLDING_TAX},
          base_currency = 'USD',
          quote_currency = json_extract(raw_data, '$.symbol'),
          quote_amount = NULL,
          description = 'Withholding (' || COALESCE(json_extract(raw_data, '$.symbol'), 'interest') || ')'
      WHERE #{alpaca_rows} AND #{activity_type_in(WITHHOLDING_TYPES)}
    SQL

    # A capital distribution reduces basis; as income it was taxed and the basis never moved.
    execute(<<~SQL.squish)
      UPDATE account_transactions
      SET entry_type = #{RETURN_OF_CAPITAL},
          base_currency = json_extract(raw_data, '$.symbol'),
          base_amount = json_extract(raw_data, '$.qty'),
          quote_currency = 'USD',
          quote_amount = json_extract(raw_data, '$.net_amount'),
          description = 'Return of capital (' || json_extract(raw_data, '$.symbol') || ')'
      WHERE #{alpaca_rows}
        AND json_extract(raw_data, '$.activity_type') = 'DIVROC'
        AND json_extract(raw_data, '$.symbol') IS NOT NULL
        AND json_extract(raw_data, '$.qty') IS NOT NULL
    SQL

    execute(<<~SQL.squish)
      UPDATE account_transactions
      SET entry_type = #{OTHER_INCOME},
          base_currency = 'USD',
          quote_currency = NULL,
          quote_amount = NULL,
          description = NULL
      WHERE #{alpaca_rows} AND #{activity_type_in(INTEREST_TYPES)}
    SQL

    # A crypto fill kept the raw pair as its base currency ("BTC/USD" or the compact "BTCUSD"), and
    # `Tax::CryptoScope#crypto?` resolves a symbol, never a pair. Stock fills already store the bare
    # symbol and match neither branch.
    execute(<<~SQL.squish)
      UPDATE account_transactions
      SET quote_currency = substr(base_currency, instr(base_currency, '/') + 1),
          base_currency = substr(base_currency, 1, instr(base_currency, '/') - 1)
      WHERE #{alpaca_rows}
        AND json_extract(raw_data, '$.activity_type') = 'FILL'
        AND instr(base_currency, '/') > 1
    SQL

    # The compact form needs the ticker table, exactly as `crypto_position_index` does at runtime.
    execute(<<~SQL.squish)
      UPDATE account_transactions
      SET base_currency = (
            SELECT t.base FROM tickers t
            WHERE t.exchange_id = account_transactions.exchange_id
              AND t.base || t.quote = account_transactions.base_currency
            LIMIT 1
          ),
          quote_currency = (
            SELECT t.quote FROM tickers t
            WHERE t.exchange_id = account_transactions.exchange_id
              AND t.base || t.quote = account_transactions.base_currency
            LIMIT 1
          )
      WHERE #{alpaca_rows}
        AND json_extract(raw_data, '$.activity_type') = 'FILL'
        AND EXISTS (
          SELECT 1 FROM tickers t
          WHERE t.exchange_id = account_transactions.exchange_id
            AND t.base || t.quote = account_transactions.base_currency
        )
    SQL
  end

  def down
    # Irreversible data normalization, and deliberately so: the previous labelling was wrong.
    # `raw_data` still holds the verbatim Alpaca activity if a row ever needs reconstructing.
  end

  private

  def alpaca_rows
    "exchange_id IN (SELECT id FROM exchanges WHERE type = 'Exchanges::Alpaca')"
  end

  def activity_type_in(types)
    list = types.map { |type| "'#{type}'" }.join(', ')
    "json_extract(raw_data, '$.activity_type') IN (#{list})"
  end
end

class AccountTransaction < ApplicationRecord
  require 'csv'

  belongs_to :user
  belongs_to :api_key, optional: true
  belongs_to :exchange
  belongs_to :bot_transaction, class_name: 'Transaction', foreign_key: 'transaction_id', optional: true
  belongs_to :linked_transaction, class_name: 'AccountTransaction', optional: true
  has_one :inverse_link, class_name: 'AccountTransaction', foreign_key: :linked_transaction_id,
                         inverse_of: :linked_transaction, dependent: :nullify

  enum :entry_type, {
    buy: 0, sell: 1, swap_in: 2, swap_out: 3,
    deposit: 4, withdrawal: 5, staking_reward: 6,
    lending_interest: 7, airdrop: 8, mining: 9,
    fee: 10, other_income: 11, lost: 12,
    withholding_tax: 13, return_of_capital: 14,
    adjustment: 15, unsupported_activity: 16
  }

  # The fields a user may state for themselves. Deliberately a short list: each one has to be read
  # by something downstream, and a value nothing consumes is a promise the page cannot keep.
  #
  # `price` is what one unit of the row's asset was worth, in USD — the record holds amounts and
  # prices, never a value: a value is amount times price, worked out later in whatever currency the
  # reader asks for. The app prices a row from the venue's own numbers where it can and from our
  # price history where it cannot; a stated price stands in front of both.
  MANUAL_FIELDS = %w[price].freeze

  # A user-asserted transfer link gets 14 days (not the auto-matcher's 72 hours) and no 2%
  # tolerance. Changing this also means editing the 15 tracker.transfer_no_candidate translations,
  # which spell "14 days" out rather than interpolating it.
  TRANSFER_LINK_WINDOW = 14.days

  # Whose figure this is. A number the user typed is theirs, and the row says so — a manual value
  # that looked like the exchange's would be worse than no manual value at all.
  def manual?(field) = manual_value(field).present?

  def manual_value(field)
    (manual_values || {})[field.to_s].presence&.to_d
  end

  # nil clears it and hands the row back to whatever the app works out for itself. Anything that is
  # not a number is not a value: a blank, a word, a stray keystroke leaves the field as it was.
  def set_manual(field, value)
    raise ArgumentError, "#{field} cannot be stated by hand" unless MANUAL_FIELDS.include?(field.to_s)

    number = parse_manual(value)
    raise ArgumentError, 'a row the venue valued is not yours to state' if number && venue_valued?

    self.manual_values = (manual_values || {}).except(field.to_s)
    self.manual_values = manual_values.merge(field.to_s => number.to_s) if number
    number
  end

  # A row the venue itself valued: amount and price of its own, or a cash leg beside it in its group
  # — a Convert into USDC says what the coins fetched as plainly as a quote would. Its worth is that
  # figure, and a figure typed in front of it would leave the columns no longer multiplying out, so
  # such a row takes no stated value — not written, and not read if one was written before this rule.
  def venue_valued?
    quoted? || cash_counterpart.present?
  end

  def quoted?
    quote_amount.present? && Tracker::UnfundedCash.cash?(quote_currency)
  end

  # The cash row opposite this one in a two-leg group: a swap-out's cash in-leg, a swap-in's or a
  # buy's cash out-leg. Two legs only — a sweep of many coins into one credit shares its cash out
  # by the engine's own rule, which a row on its own cannot repeat. A page of rows hands the groups
  # in (`group_rows=`); a row on its own asks for its group.
  attr_writer :group_rows

  def cash_counterpart
    return @cash_counterpart if defined?(@cash_counterpart)

    rows = group_rows
    @cash_counterpart = rows.size == 2 ? rows.find { |row| row.id != id && Tracker::UnfundedCash.cash?(row.base_currency) && opposite?(row) } : nil
  end

  validates :base_currency, presence: true
  validates :base_amount, presence: true
  validates :transacted_at, presence: true
  validates :tx_id, uniqueness: { scope: %i[user_id exchange_id] }, allow_nil: true
  validate :linked_transaction_is_valid, if: -> { linked_transaction_id.present? }

  scope :for_user, ->(user) { where(user_id: user.id) }
  scope :for_exchange, ->(exchange) { where(exchange_id: exchange.id) }
  scope :by_date, -> { order(transacted_at: :desc) }
  scope :by_date_asc, -> { order(transacted_at: :asc) }
  scope :in_date_range, lambda { |from, to|
    scope = all
    scope = scope.where(transacted_at: from..) if from.present?
    scope = scope.where(transacted_at: ..to) if to.present?
    scope
  }

  # Blank, a word, a stray keystroke: none of them are a value. Public because the currency has to be
  # converted between reading the box and storing the figure, and only a number can be converted.
  def parse_manual(value)
    return nil if value.nil? || value.to_s.strip.empty?

    number = BigDecimal(value.to_s.strip)
    number.negative? ? nil : number
  rescue ArgumentError, TypeError
    nil
  end

  def self.csv_headers
    %w[date type base_currency base_amount quote_currency quote_amount fee_currency fee_amount exchange tx_id group_id description]
  end

  def self.to_csv(records)
    CsvSafe.generate do |csv|
      csv << csv_headers
      records.order(transacted_at: :asc).each do |record|
        csv << record.to_csv_row
      end
    end
  end

  def to_csv_row
    [
      transacted_at.utc.strftime('%Y-%m-%dT%H:%M:%SZ'),
      entry_type,
      base_currency,
      base_amount,
      quote_currency,
      quote_amount,
      fee_currency,
      fee_amount,
      exchange.name_id,
      tx_id,
      group_id,
      description
    ]
  end

  def linked? = linked_transaction_id.present? || inverse_link.present?

  # At most two rows: one is a link, two is an ambiguity the caller has to report.
  def transfer_candidates(user)
    scope = AccountTransaction.for_user(user).where(base_currency: base_currency)
    at = transacted_at

    if withdrawal?
      # Claimed deposits must be excluded before update! reaches the unique index.
      claimed = AccountTransaction.for_user(user).where.not(linked_transaction_id: nil).select(:linked_transaction_id)
      scope.deposit
           .where(transacted_at: at..(at + TRANSFER_LINK_WINDOW))
           .where(base_amount: ..base_amount)
           .where.not(id: claimed)
           .limit(2).to_a
    elsif deposit?
      scope.withdrawal
           .where(linked_transaction_id: nil)
           .where(transacted_at: (at - TRANSFER_LINK_WINDOW)..at)
           .where(base_amount: base_amount..)
           .limit(2).to_a
    else
      []
    end
  end

  private

  IN_LEGS = %w[buy swap_in].freeze
  OUT_LEGS = %w[sell swap_out].freeze

  def group_rows
    @group_rows ||= group_id.blank? ? [] : AccountTransaction.where(user_id: user_id, exchange_id: exchange_id, group_id: group_id).to_a
  end

  def opposite?(row)
    (IN_LEGS.include?(entry_type) && OUT_LEGS.include?(row.entry_type)) ||
      (OUT_LEGS.include?(entry_type) && IN_LEGS.include?(row.entry_type))
  end

  def linked_transaction_is_valid
    linked = linked_transaction
    return errors.add(:linked_transaction, :not_found) unless linked

    errors.add(:linked_transaction, :different_user) unless linked.user_id == user_id
    errors.add(:linked_transaction, :different_currency) unless linked.base_currency == base_currency
    errors.add(:linked_transaction, :wrong_direction) unless withdrawal? && linked.deposit?
    errors.add(:linked_transaction, :before_withdrawal) if transacted_at.present? && linked.transacted_at < transacted_at
    # A transfer can only shrink by its network fee. Were the deposit bigger, it would contribute no
    # lot while the withdrawal shrank nothing, silently vaporising the difference in cost basis.
    return unless base_amount.present? && linked.base_amount > base_amount

    errors.add(:linked_transaction, :larger_than_withdrawal)
  end
end

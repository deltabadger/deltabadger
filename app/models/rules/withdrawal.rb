class Rules::Withdrawal < Rule
  belongs_to :exchange
  belongs_to :asset

  encrypts :address

  store_accessor :settings, :max_fee_percentage

  validates :address, presence: true
  validates :max_fee_percentage, presence: true,
                                 numericality: { greater_than: 0, less_than_or_equal_to: 100 }

  def minimum_withdrawal_amount
    return nil if max_fee_percentage.blank?

    fee = BigDecimal(exchange.withdrawal_fee.presence || '0')
    pct = BigDecimal(max_fee_percentage.to_s)
    return nil if fee.zero? || pct.zero?

    (fee / (pct / 100)).round(8)
  end
end

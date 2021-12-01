class PaymentsRepository < BaseRepository
  def paid
    model.where(status: :paid)
  end

  # Returns payments paid between from and to (UTC, inclusive)
  def paid_between(from:, to:, wire:)
    from = from.blank? ? Date.new(0) : Date.parse(from)
    to = to.blank? ? Date.tomorrow : Date.parse(to) + 1.day
    if wire
      paid.where(paid_at: from..to, crypto_paid: 0.0)
    else
      paid.where(paid_at: from..to).where.not(crypto_paid: 0.0)
    end
  end

  def model
    Payment
  end
end

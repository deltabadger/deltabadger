class PaymentsRepository < BaseRepository
  def paid
    model.where(status: :paid)
  end

  # Returns payments paid between from and to (UTC, inclusive)
  def paid_between(from:, to:, fiat:)
    from = from.blank? ? Date.new(0) : Date.parse(from)
    to = to.blank? ? Date.tomorrow : Date.parse(to) + 1.day
    paid.where(paid_at: from..to, payment_type: fiat ? %w[stripe zen wire] : 'bitcoin')
  end

  def model
    Payment
  end
end

class AffiliatesRepository < BaseRepository
  def model
    Affiliate
  end

  def find_active_by_code(code)
    affiliate = model.active.find_by(code: code.upcase)
    return unless affiliate&.user&.unlimited?

    affiliate
  end

  def active?(id:)
    affiliate = model.active.where(id: id).first

    affiliate&.user&.unlimited?
  end

  def total_unexported
    model.sum(:unexported_crypto_commission)
  end

  def total_exported
    model.sum(:exported_crypto_commission)
  end

  def total_paid
    model.sum(:paid_crypto_commission)
  end
end

class AffiliatesRepository < BaseRepository
  def model
    Affiliate
  end

  def find_active_by_code(code)
    return nil unless code

    affiliate = find_affiliate(code.upcase)
    return nil unless affiliate&.user.present?

    affiliate
  end

  def get_code_presenter(code)
    return nil if code.nil?

    affiliate = find_active_by_code(code)
    code_presenter = Presenters::RefCodes::Show.new(affiliate)

    code_presenter
  end

  def active?(id:)
    affiliate = model.active.where(id: id).first

    affiliate&.user&.unlimited?
  end

  def all_with_unpaid_commissions
    model.includes(:user).where('exported_crypto_commission > 0')
  end

  def mark_all_exported_commissions_as_paid
    model.update_all(
      'paid_crypto_commission = paid_crypto_commission + exported_crypto_commission, '\
      'exported_crypto_commission = 0'
    )
  end

  def total_waiting
    model.where(btc_address: [nil, ''])
         .sum(:unexported_crypto_commission)
  end

  def total_unexported
    model.where.not(btc_address: [nil, ''])
         .sum(:unexported_crypto_commission)
  end

  def total_exported
    model.sum(:exported_crypto_commission)
  end

  def total_paid
    model.sum(:paid_crypto_commission)
  end

  private

  def find_affiliate(code)
    affiliate = model.active.find_by(code: code)
    return affiliate if affiliate.present?

    model.active.find_by(old_code: code)
  end
end

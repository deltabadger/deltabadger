class AffiliatesRepository < BaseRepository
  def model
    Affiliate
  end

  def find_active_by_code(code)
    affiliate = Affiliate.active.find_by(code: code)
    return unless affiliate&.user&.unlimited?

    affiliate
  end

  def active?(id:)
    affiliate = Affiliate.active.where(id: id).first

    affiliate&.user&.unlimited?
  end
end

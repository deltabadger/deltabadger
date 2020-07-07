class EuCompanyAffiliate < Affiliate
  validates :name, :address, presence: true
end

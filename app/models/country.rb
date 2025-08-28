class Country < ApplicationRecord
  after_commit :reset_all_in_display_order_cache

  validates :vat_rate, numericality: { greater_than_or_equal_to: 0, less_than: 1 }
  validates :name, presence: true, uniqueness: true
  validates :code, uniqueness: true

  scope :eu_countries, -> { where(eu_member: true) }
  scope :non_eu_countries, -> { where(eu_member: false) }

  enum currency: %i[usd eur] # must match Payment.currency

  def self.all_in_display_order
    @all_in_display_order ||= begin
      eu_in_alphabetical_order = eu_countries.order(name: :asc)
      other_in_alphabetical_order = non_eu_countries.order(name: :asc)
      other_in_alphabetical_order + eu_in_alphabetical_order
    end
  end

  def self.reset_all_in_display_order_cache
    @all_in_display_order = nil
  end

  private

  def reset_all_in_display_order_cache
    self.class.reset_all_in_display_order_cache
  end
end

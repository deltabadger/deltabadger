class HistoricalPrice < ApplicationRecord
  validates :asset, :currency, :date, :price, presence: true
  validates :asset, uniqueness: { scope: %i[currency date] }

  def self.lookup(asset:, currency:, date:)
    find_by(asset: asset, currency: currency, date: date)&.price
  end

  def self.store(asset:, currency:, date:, price:)
    insert({ asset: asset, currency: currency, date: date, price: price },
           unique_by: %i[asset currency date])
  end

  def self.bulk_store(records)
    insert_all(records, unique_by: %i[asset currency date]) if records.any?
  end
end

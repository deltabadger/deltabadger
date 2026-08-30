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

  # A high-water mark for everything computed FROM these prices. Rows are immutable reference data
  # and are only ever inserted, so the largest id moves exactly when a price arrives and stops
  # moving when the gaps that can be filled are filled — which is what keeps the things keyed on it
  # from redoing themselves forever over a date nobody can price. An index lookup, not a count.
  def self.generation
    maximum(:id).to_i
  end
end

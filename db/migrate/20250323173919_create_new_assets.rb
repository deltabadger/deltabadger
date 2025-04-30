class CreateNewAssets < ActiveRecord::Migration[6.0]
  def change
    create_table :assets do |t|
      t.string :external_id, null: false
      t.string :symbol
      t.string :name
      t.string :isin
      t.string :color
      t.string :category
      t.string :country
      t.string :country_exchange
      t.string :url
      t.string :image_url
      t.integer :market_cap_rank
      t.timestamps

      t.index :external_id, unique: true
      t.index :symbol
      t.index :name
      t.index :isin
    end
  end
end


# class Asset < ApplicationRecord
#   validates :external_id, presence: true, uniqueness: true
# end



# class PortfolioAssets < ApplicationRecord
#   has_many :assets, foreign_key: "portfolio_assets_id", primary_key: "external_id", dependent: :destroy
# end

# class Asset < ApplicationRecord
#   belongs_to :portfolio_assets, foreign_key: "portfolio_assets_id", primary_key: "external_id"
# end

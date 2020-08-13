class AddVisibleLinkSchemeToAffiliates < ActiveRecord::Migration[5.2]
  def change
    add_column :affiliates, :visible_link_scheme, :string, null: false, default: 'https'
  end
end

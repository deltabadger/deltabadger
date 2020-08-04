class CreateVatRates < ActiveRecord::Migration[5.2]
  def change
    create_table :vat_rates do |t|
      t.string :country, null: false
      t.decimal :vat, precision: 2, scale: 2, null: false
      t.timestamps
    end

    reversible do |dir|
      dir.up do
        execute <<~SQL
          INSERT INTO vat_rates(country, vat, created_at, updated_at)
            VALUES
              ('Other', 0, NOW(), NOW()),
              ('Austria',0.2, NOW(), NOW()),
              ('Belgium',0.21, NOW(), NOW()),
              ('Bulgaria',0.2, NOW(), NOW()),
              ('Croatia',0.25, NOW(), NOW()),
              ('Cyprus',0.19, NOW(), NOW()),
              ('Czech Republic',0.21, NOW(), NOW()),
              ('Denmark',0.25, NOW(), NOW()),
              ('Estonia',0.2, NOW(), NOW()),
              ('Finland',0.24, NOW(), NOW()),
              ('France',0.2, NOW(), NOW()),
              ('Germany',0.19, NOW(), NOW()),
              ('Greece',0.24, NOW(), NOW()),
              ('Hungary',0.27, NOW(), NOW()),
              ('Ireland',0.23, NOW(), NOW()),
              ('Italy',0.22, NOW(), NOW()),
              ('Latvia',0.21, NOW(), NOW()),
              ('Lithuania',0.21, NOW(), NOW()),
              ('Luxembourg',0.17, NOW(), NOW()),
              ('Malta',0.18, NOW(), NOW()),
              ('Netherlands',0.21, NOW(), NOW()),
              ('Poland',0.23, NOW(), NOW()),
              ('Portugal',0.23, NOW(), NOW()),
              ('Romania',0.19, NOW(), NOW()),
              ('Slovakia',0.2, NOW(), NOW()),
              ('Slovenia',0.22, NOW(), NOW()),
              ('Spain',0.21, NOW(), NOW()),
              ('Sweden',0.25, NOW(), NOW()),
              ('United Kingdom',0.2, NOW(), NOW())
        SQL
      end
    end
  end
end

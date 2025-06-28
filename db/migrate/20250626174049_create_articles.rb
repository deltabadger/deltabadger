class CreateArticles < ActiveRecord::Migration[6.0]
  def change
    create_table :articles do |t|
      t.string :slug, null: false
      t.string :locale, null: false, limit: 2
      t.string :title, null: false
      t.text :excerpt
      t.text :content, null: false
      t.boolean :published, default: false, null: false
      t.datetime :published_at
      t.string :author_name
      t.string :author_email
      t.string :meta_description
      t.string :meta_keywords
      t.integer :reading_time_minutes
      t.string :paywall_marker, default: '<!-- PAYWALL -->'
      t.timestamps
    end

    add_index :articles, [:slug, :locale], unique: true
    add_index :articles, [:published, :published_at]
    add_index :articles, :locale
  end
end

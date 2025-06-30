class CreateArticles < ActiveRecord::Migration[6.0]
  def change
    create_table :articles do |t|
      t.string :slug, null: false
      t.string :locale, null: false, limit: 2
      t.string :title, null: false
      t.string :subtitle
      t.text :excerpt
      t.text :content, null: false
      t.string :thumbnail
      t.references :author, null: true, foreign_key: true
      t.integer :reading_time_minutes
      t.boolean :published, default: false, null: false
      t.datetime :published_at
      t.timestamps
    end

    add_index :articles, [:slug, :locale], unique: true
    add_index :articles, [:published, :published_at]
    add_index :articles, :locale
  end
end

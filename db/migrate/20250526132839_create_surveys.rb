class CreateSurveys < ActiveRecord::Migration[6.0]
  def change
    create_table :surveys do |t|
      t.references :user, null: false, foreign_key: true
      t.string :type, null: false
      t.jsonb :answers, null: false, default: {}
      t.timestamps

      t.index [:user_id, :type], unique: true
    end
  end
end

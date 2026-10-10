use super::*;
    fn offer(redeploy:bool)->Option<Dec>{
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE bots(id INTEGER,redeploy_declined_offset DECIMAL); INSERT INTO bots VALUES(1,0);
            CREATE TABLE tickers(id INTEGER,minimum_quote_size DECIMAL); INSERT INTO tickers VALUES(1,1);
            CREATE TABLE bot_index_assets(bot_id INTEGER,ticker_id INTEGER,in_index INTEGER); INSERT INTO bot_index_assets VALUES(1,1,1);
            CREATE TABLE transactions(id INTEGER,bot_id INTEGER,status INTEGER,created_at TEXT,exchange_id INTEGER,price DECIMAL,amount DECIMAL,amount_exec DECIMAL,quote_amount_exec DECIMAL,base TEXT,base_asset_id INTEGER,side INTEGER,external_status INTEGER,transaction_type TEXT);
            INSERT INTO transactions VALUES(1,1,0,'2026-03-02 00:00:00',1,110,1,1,NULL,'AAA',2,1,3,'LIQUIDATION');").unwrap();
        if redeploy {c.execute_batch("UPDATE transactions SET quote_amount_exec=110 WHERE id=1;
            INSERT INTO transactions VALUES(2,1,0,'2026-03-02 00:01:00',1,100,1,0.5,NULL,'AAA',2,0,2,'REDEPLOY');").unwrap();}
        let s=Subject{bot:db::Bot{id:1,user_id:1,exchange_id:Some(1),kind:db::Kind::Basket,exchange_type:Some("Exchanges::Alpaca".into()),quote_asset_id:Some(1),base_asset_ids:vec![2]},orders:db::orders(&c,1).unwrap(),tickers:vec![],quote:Some("USD".into())};
        let mut m=Metrics::empty();
        m.walked=Some(crate::figures::walk::Walked{shadowed_by:vec![],external_sales:false,restated_at:None,restated_before_external_sales:false,realised_cash:Num::Int(if redeploy{60}else{110}),asset_lots:vec![],tax_pnl_by_transaction:vec![],loss_lot_by_transaction:vec![]});
        let offer=redeploy_offer(&c,&s,&m,&HashSet::new()).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM transactions WHERE quote_amount_exec IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),1,"normalization does not write stored columns");
        offer
    }
    #[test]
    // RULING-B2B-1 MQ6: an unpriced liquidation banks nothing (Redeployable#redeploy_banked sums reported proceeds).
    fn redeploy_banks_only_reported_liquidation_proceeds(){assert_eq!(offer(false),Some(Dec::zero()));}
    #[test]
    fn redeploy_uses_normalized_spend_at_effective_quantity(){assert_eq!(offer(true),Some(Dec::from_i64(60)));}

# Ledgit

I need you to create a version-controlled Windows budget app for me. Use Rust. Follow data-oriented design principles.

The app will consist of a core library that wraps the database and basic data structures in an api.

The app will version-control the budget in a user friendly way.

The app will provide a GUI.

Finally, it will provide an installer app. 

## 1. Core Library - Datastructures and API

Provides a database, data structures, and an API to manuipulate them with.

### 1. Database

Select a database that plays nice with Windows, but interface it out so it can be swappable with something better for Linux in the future.
Or, choose one that plays nice with both to start. Your call. Select for data durability.

### 2. Data Structures

These are the basic data structures I'd like to manuipulate with my budget. None of these data structures can be deleted unless otherwise noted.

1. Ledger - A basic ledger with a balance, a normality (Credit/Debit), a name, and a description.
Ledgers can never be deleted. The balance may be only modified with Transactions. The name and description are non-const.

2. Transaction - A record that moves money between ledgers. A transaction has:
    - A creditted ledger
    - A debitted ledger
    - A date
    - A name
    - A description
    - A "parent", either an Issuer (more on that later) or a special case of "Manual", which means it was created by me using the app.
Once created, all transaction fields except the name and description are const. 

3. Issuer - An entity that creates transactions at regular intervals. For example, I could have one that credits my cash ledger and debits my car loan by $400 every two weeks.
I can select a recurrence interval with the granularity of a day. Issuers can be paused (i.e. stopped from creating new transactions), but not deleted.

4. Bucket - A list of ledgers that are related in some way. Ledgers can be in multiple buckets. For example, "Net Worth" might be a bucket that contains all ledgers that contribute to my net worth. 
I can run queries on a bucket like calculating cumulative balances (with the option to add/subtract based on ledger normality), sort by name, normality, balances, and date created. Buckets can be created and deleted, since they are simple views and should not affect any balance sheet.

5. Querier - A query is a context structure passed to the API to retrieve any of the above structures. It should look something like the below (excuse my psuedo-Rust):

```Rust
enum LedgerValue {
    Balance(i32),
    Normality(enum Norm),
    Date(Date),
    Name(str),
    Desc(str)
};

enum TransactionValue { ... };

... 

trait QueryTarget;

impl ContextTarget for LedgerValue, TransactionTarget ...

enum QueryTarget {
    Ledger(enum LedgerValue),
    Transaction(enum TransactionTarget),
    Issuer,
    Bucket,
}

impl ContextTarget for QueryTarget;

enum ReturnValue {
    Int(i32),
    Str(str),
    Slice(Rc<[T]>),
    ...
}

struct Query {
    target: enum QueryTarget;
    comparator: *** Some way of comparing the target things ***;
    action: Option<F> 
    where
        F: Fn(Option<ContextTarget>) -> Result<ReturnValue>;
}

```

6. Writer - A Writer is a context structure used to manuipulate the above data structures. Similar to the query object above

The Query and Writer object allow the front end to build features using closures and comparators without needing to implement every front end functionality in the core library.
If you hate this architecture, let me know what you think is better, or maybe there is a better way of implementing this.

### 3. API

The API should expose the ability to create and read ledgers, create transactions, create or pause issuers, and create and delete buckets. It should consume query and writer objects:

```Rust

fn query(Vec<Query>) -> Result<ReturnValue, Err>;

fn write(Vec<Writer>) -> Result<ReturnValue, Err>;

```

## 2. Version Control

Everything is done in commits exactly like Git. 
If I screw up a transaction, I'll need to undo it by just adding transactions in "reverse", rather than deleting ledgers or issuers.

## 3. GUI - See .claude/GUI.md for more

Create a GUI. Use whatever framework you'd like.
Have a dashboard where i can view buckets and pin important ledgers.
Have a search bar where I can search for transactions, issuers, and ledgers and buckets.
Give me lots of data reading and analysis power.
Give me the ability to create transactions, ledgers, buckets, and issuers.
Nothing is written to disk until i press commit. Before I press commit, give me report of everything I've done and every bucket that might be affected by my transactions.
Give me a GUI to look at the work tree, revert, branch, and rebase the budget if needed.
Give me the option to look at all the transactions created by issuers since last commit. 

## 4. Installer

Create a basic installer to install the app on Windows, however I am comfortable using cargo to build the app as well.
